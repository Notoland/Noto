//! Running the phases in order.

use noto_ast::Module;
use noto_codegen::Target;
use noto_semantic::ModuleInput;
use noto_diagnostics::{codes, Diagnostic, DiagnosticSink};
use noto_ir::Program;
use noto_semantic::Analysis;
use noto_span::{FileId, SourceMap};

/// How far a compilation should go.
///
/// Each stage runs everything before it, so `noto check` and `noto build`
/// share exactly the same front end.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Stage {
    /// Parse only.
    Parse,
    /// Parse and analyse. What `noto check` runs.
    Check,
    /// Produce Noto IR.
    Ir,
    /// Produce a native executable.
    Executable,
}

/// Settings for one compilation.
#[derive(Clone, Debug)]
pub struct CompileOptions {
    /// How far to go.
    pub stage: Stage,
    /// What to generate code for.
    pub target: Target,
    /// Whether to run the optimizer.
    pub optimize: bool,
    /// Whether a missing `main` is acceptable.
    ///
    /// It is an error for `noto build` and `noto run`; the test runner sets
    /// this because a file of tests is legitimate without one.
    pub allow_no_main: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            stage: Stage::Executable,
            target: Target::host(),
            optimize: true,
            allow_no_main: false,
        }
    }
}

/// What a compilation produced.
///
/// Each field is present only if its phase ran and the ones before it
/// succeeded, so a caller can tell how far compilation got.
#[derive(Default)]
pub struct Compilation {
    /// Every parsed module, the root first.
    pub modules: Vec<Module>,
    /// What each module imports, in the same order.
    pub imports: Vec<Vec<noto_semantic::Import>>,
    /// The file the root module was read from.
    pub root_file: Option<FileId>,
    /// What analysis learned.
    pub analysis: Option<Analysis>,
    /// The lowered program.
    pub ir: Option<Program>,
    /// The bytes of the executable.
    pub executable: Option<Vec<u8>>,
}

impl Compilation {
    /// Whether an executable was produced.
    pub fn is_complete(&self) -> bool {
        self.executable.is_some()
    }

    /// The root module: the file the compiler was pointed at.
    pub fn module(&self) -> Option<&Module> {
        self.modules.first()
    }

    /// What the root module imports.
    pub fn root_imports(&self) -> &[noto_semantic::Import] {
        self.imports.first().map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Compiles a program: the file at `root` and everything it imports.
///
/// This is what every command but `noto fmt` runs. Imports are resolved
/// first, so a missing module or a cycle is reported before anything is
/// checked; then the whole program is analysed at once, because a call may
/// cross a module boundary in either direction.
pub fn compile_path(
    map: &mut SourceMap,
    root: &std::path::Path,
    options: &CompileOptions,
    sink: &mut DiagnosticSink,
) -> Compilation {
    let mut result = Compilation::default();
    let Some(loaded) = crate::modules::load(map, root, sink) else { return result };

    result.imports =
        loaded.modules.iter().map(|module| module.imports.clone()).collect();
    result.root_file = loaded.modules.first().map(|module| module.file);

    let stop = options.stage == Stage::Parse || sink.has_errors();
    if stop {
        result.modules = loaded.modules.into_iter().map(|module| module.ast).collect();
        return result;
    }

    let inputs: Vec<ModuleInput> = loaded
        .modules
        .iter()
        .map(|module| ModuleInput {
            name: &module.name,
            ast: &module.ast,
            imports: &module.imports,
        })
        .collect();
    let analysis = noto_semantic::analyze_program(&inputs, sink);
    drop(inputs);

    let root_name = loaded.modules[0].file;
    check_entry(&analysis, map, root_name, options, sink);

    if options.stage == Stage::Check || sink.has_errors() {
        result.modules = loaded.modules.into_iter().map(|module| module.ast).collect();
        result.analysis = Some(analysis);
        return result;
    }

    let asts: Vec<&Module> = loaded.modules.iter().map(|module| &module.ast).collect();
    let mut program = noto_lower::lower_program(&asts, &analysis, sink);
    drop(asts);
    result.modules = loaded.modules.into_iter().map(|module| module.ast).collect();
    result.analysis = Some(analysis);

    if sink.has_errors() {
        result.ir = Some(program);
        return result;
    }

    if options.optimize {
        noto_optimizer::optimize(&mut program);
    }
    result.ir = Some(program);
    if options.stage == Stage::Ir {
        return result;
    }

    let program = result.ir.as_ref().expect("just set");
    match noto_codegen::compile(program, options.target) {
        Ok(executable) => result.executable = Some(executable),
        Err(error) => sink.emit(error.to_diagnostic()),
    }

    result
}

/// Reports a program that cannot be run because it has no entry point.
fn check_entry(
    analysis: &noto_semantic::Analysis,
    map: &SourceMap,
    file: FileId,
    options: &CompileOptions,
    sink: &mut DiagnosticSink,
) {
    if options.stage < Stage::Ir || analysis.entry.is_some() || options.allow_no_main {
        return;
    }
    let name = map.file(file).map(|file| file.name().to_string()).unwrap_or_default();
    sink.emit(
        Diagnostic::error(codes::NO_MAIN, "this program has no `main` function")
            .with_note(format!("`{name}` declares no entry point"))
            .with_help("add `fn main() { ... }`"),
    );
}

/// Compiles one already-loaded source file.
pub fn compile(
    map: &SourceMap,
    file: FileId,
    options: &CompileOptions,
    sink: &mut DiagnosticSink,
) -> Compilation {
    let mut result = Compilation::default();

    let Some(source) = map.file(file) else {
        sink.emit(Diagnostic::fatal(codes::CANNOT_READ_FILE, "the source file is not loaded"));
        return result;
    };

    let module = noto_parser::parse_file(source, sink);
    let stop = options.stage == Stage::Parse || sink.has_errors();
    result.modules = vec![module];
    result.imports = vec![Vec::new()];
    result.root_file = Some(file);
    if stop {
        return result;
    }

    let module = result.modules.first().expect("just set");
    let analysis = noto_semantic::analyze(module, sink);

    // A program without `main` cannot be run, but it can still be checked, so
    // this is only an error once code generation is asked for.
    if options.stage >= Stage::Ir && analysis.entry.is_none() && !options.allow_no_main {
        sink.emit(
            Diagnostic::error(codes::NO_MAIN, "this program has no `main` function")
                .with_note(format!("`{}` declares no entry point", source.name()))
                .with_help("add `fn main() { ... }`"),
        );
    }

    result.analysis = Some(analysis);
    if options.stage == Stage::Check || sink.has_errors() {
        return result;
    }

    let analysis = result.analysis.as_ref().expect("just set");
    let module = result.modules.first().expect("parsed above");
    let mut program = noto_lower::lower(module, analysis, sink);
    if sink.has_errors() {
        result.ir = Some(program);
        return result;
    }

    if options.optimize {
        noto_optimizer::optimize(&mut program);
    }

    result.ir = Some(program);
    if options.stage == Stage::Ir {
        return result;
    }

    let program = result.ir.as_ref().expect("just set");
    match noto_codegen::compile(program, options.target) {
        Ok(executable) => result.executable = Some(executable),
        Err(error) => sink.emit(error.to_diagnostic()),
    }

    result
}

/// Compiles source text held in memory, which is what the tests use.
pub fn compile_source(
    name: &str,
    source: &str,
    options: &CompileOptions,
    sink: &mut DiagnosticSink,
) -> (SourceMap, Compilation) {
    let mut map = SourceMap::new();
    let file = map.add(name, source);
    let compilation = compile(&map, file, options, sink);
    (map, compilation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(stage: Stage) -> CompileOptions {
        CompileOptions { stage, ..CompileOptions::default() }
    }

    #[test]
    fn a_check_run_stops_before_code_generation() {
        let mut sink = DiagnosticSink::new();
        let (_, compilation) = compile_source(
            "a.noto",
            "fn main() { println(\"hi\") }\n",
            &options(Stage::Check),
            &mut sink,
        );
        assert!(compilation.analysis.is_some());
        assert!(compilation.ir.is_none());
        assert!(!sink.has_errors());
    }

    #[test]
    fn a_missing_main_is_only_an_error_when_building() {
        let source = "fn helper(): Int = 1\n";

        let mut sink = DiagnosticSink::new();
        compile_source("a.noto", source, &options(Stage::Check), &mut sink);
        assert!(!sink.has_errors(), "checking a library is fine");

        let mut sink = DiagnosticSink::new();
        compile_source("a.noto", source, &options(Stage::Executable), &mut sink);
        assert!(sink.diagnostics().iter().any(|d| d.message.contains("no `main` function")));
    }

    #[test]
    fn the_test_runner_may_build_a_file_without_main() {
        let mut sink = DiagnosticSink::new();
        let (_, compilation) = compile_source(
            "a.noto",
            "test \"adds\" {\n    assert(1 + 1 == 2)\n}\n",
            &CompileOptions { allow_no_main: true, ..CompileOptions::default() },
            &mut sink,
        );
        assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
        assert!(compilation.is_complete(), "a file of tests still produces an executable");
    }

    #[test]
    fn a_syntax_error_stops_before_analysis() {
        let mut sink = DiagnosticSink::new();
        let (_, compilation) =
            compile_source("a.noto", "fn main( {\n", &options(Stage::Executable), &mut sink);
        assert!(sink.has_errors());
        assert!(compilation.analysis.is_none(), "analysis must not run on a broken tree");
    }

    #[test]
    fn a_type_error_stops_before_lowering() {
        let mut sink = DiagnosticSink::new();
        let (_, compilation) = compile_source(
            "a.noto",
            "fn main() {\n    val n: Int = \"text\"\n}\n",
            &options(Stage::Executable),
            &mut sink,
        );
        assert!(sink.has_errors());
        assert!(compilation.ir.is_none());
    }

    #[test]
    fn a_program_with_objects_reaches_the_backend() {
        let mut sink = DiagnosticSink::new();
        let (_, compilation) = compile_source(
            "point.noto",
            "class Point(val x: Int, var y: Int)\n             fn main() {\n    val p = Point(1, 2)\n    p.y = 3\n    println(p.x + p.y)\n}\n",
            &CompileOptions::default(),
            &mut sink,
        );
        assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
        assert!(compilation.is_complete());
    }

    #[test]
    fn a_complete_build_produces_an_executable() {
        let mut sink = DiagnosticSink::new();
        let (_, compilation) = compile_source(
            "hello.noto",
            "fn main() {\n    println(\"Hello, Noto!\")\n}\n",
            &CompileOptions::default(),
            &mut sink,
        );
        assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
        assert!(compilation.is_complete());
        assert_eq!(&compilation.executable.unwrap()[..4], b"\x7FELF");
    }
}

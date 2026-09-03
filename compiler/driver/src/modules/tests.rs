//! Module loading tests.
//!
//! Each test builds a real directory: what is being tested is how a file
//! path becomes a module name and back, which a fake file system would only
//! restate.

use super::*;
use noto_span::SourceMap;

/// Writes `files` into a fresh directory and loads the first one as root.
fn load_program(files: &[(&str, &str)]) -> (SourceMap, DiagnosticSink, Option<Program>) {
    let root = std::env::temp_dir().join(format!(
        "noto-modules-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));

    for (name, text) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the directory");
        std::fs::write(&path, text).expect("write the file");
    }

    let mut map = SourceMap::new();
    let mut sink = DiagnosticSink::new();
    let program = load(&mut map, &root.join(files[0].0), &mut sink);

    let _ = std::fs::remove_dir_all(&root);
    (map, sink, program)
}

fn messages(sink: &DiagnosticSink) -> Vec<String> {
    sink.diagnostics().iter().map(|d| d.message.clone()).collect()
}

#[test]
fn the_root_is_module_zero_and_has_no_name() {
    let (_, sink, program) = load_program(&[("main.noto", "fn main() {}\n")]);
    let program = program.expect("the root loads");
    assert!(!sink.has_errors(), "{:?}", messages(&sink));
    assert_eq!(program.modules.len(), 1);
    assert_eq!(program.module(ModuleId::ROOT).name, "");
}

#[test]
fn an_import_is_read_from_a_path_that_matches_its_name() {
    let (_, sink, program) = load_program(&[
        ("main.noto", "import geometry.point\nfn main() {}\n"),
        ("geometry/point.noto", "export fn distance(): Int = 1\n"),
    ]);
    let program = program.expect("loads");
    assert!(!sink.has_errors(), "{:?}", messages(&sink));
    assert_eq!(program.modules.len(), 2);
    assert_eq!(program.modules[1].name, "geometry.point");
}

#[test]
fn a_plain_import_binds_the_last_segment() {
    let (_, _, program) = load_program(&[
        ("main.noto", "import geometry.point\nfn main() {}\n"),
        ("geometry/point.noto", "export fn distance(): Int = 1\n"),
    ]);
    let program = program.expect("loads");
    assert_eq!(program.module(ModuleId::ROOT).imports[0].binding.as_deref(), Some("point"));
}

#[test]
fn an_alias_renames_the_namespace() {
    let (_, _, program) = load_program(&[
        ("main.noto", "import geometry.point as geo\nfn main() {}\n"),
        ("geometry/point.noto", "export fn distance(): Int = 1\n"),
    ]);
    let program = program.expect("loads");
    assert_eq!(program.module(ModuleId::ROOT).imports[0].binding.as_deref(), Some("geo"));
}

#[test]
fn a_selective_import_binds_no_namespace() {
    let (_, _, program) = load_program(&[
        ("main.noto", "import util { helper }\nfn main() {}\n"),
        ("util.noto", "export fn helper(): Int = 1\n"),
    ]);
    let program = program.expect("loads");
    let import = &program.module(ModuleId::ROOT).imports[0];
    assert_eq!(import.binding, None);
    assert_eq!(import.names.len(), 1);
    assert_eq!(import.names[0].name, "helper");
}

#[test]
fn a_module_imported_twice_is_loaded_once() {
    let (_, sink, program) = load_program(&[
        ("main.noto", "import util\nimport a\nfn main() {}\n"),
        ("util.noto", "export fn helper(): Int = 1\n"),
        ("a.noto", "import util\nexport fn other(): Int = 1\n"),
    ]);
    let program = program.expect("loads");
    assert!(!sink.has_errors(), "{:?}", messages(&sink));
    assert_eq!(program.modules.len(), 3, "util is one module however often it is named");
}

#[test]
fn a_missing_module_names_the_file_it_looked_for() {
    let (_, sink, _) = load_program(&[("main.noto", "import nowhere\nfn main() {}\n")]);
    assert!(sink.has_errors());
    let rendered = format!("{:?}", sink.diagnostics());
    assert!(rendered.contains("cannot find module `nowhere`"), "{rendered}");
    assert!(rendered.contains("nowhere.noto"), "the path it expected: {rendered}");
}

#[test]
fn every_missing_module_is_reported_not_only_the_first() {
    let (_, sink, _) = load_program(&[(
        "main.noto",
        "import nowhere\nimport neither\nfn main() {}\n",
    )]);
    assert_eq!(sink.error_count(), 2, "{:?}", messages(&sink));
}

#[test]
fn a_cycle_is_reported_with_the_modules_that_form_it() {
    let (_, sink, _) = load_program(&[
        ("main.noto", "import a\nfn main() {}\n"),
        ("a.noto", "import b\nexport fn one(): Int = 1\n"),
        ("b.noto", "import a\nexport fn two(): Int = 2\n"),
    ]);
    assert!(sink.has_errors());
    let rendered = format!("{:?}", sink.diagnostics());
    assert!(rendered.contains("imported in a cycle"), "{rendered}");
    assert!(rendered.contains("a -> b"), "the cycle is spelled out: {rendered}");
}

#[test]
fn a_module_importing_itself_is_a_cycle() {
    let (_, sink, _) = load_program(&[
        ("main.noto", "import a\nfn main() {}\n"),
        ("a.noto", "import a\nexport fn one(): Int = 1\n"),
    ]);
    assert!(sink.has_errors());
    assert!(messages(&sink).iter().any(|m| m.contains("cycle")), "{:?}", messages(&sink));
}

#[test]
fn a_diamond_is_not_a_cycle() {
    let (_, sink, program) = load_program(&[
        ("main.noto", "import left\nimport right\nfn main() {}\n"),
        ("left.noto", "import shared\nexport fn one(): Int = 1\n"),
        ("right.noto", "import shared\nexport fn two(): Int = 2\n"),
        ("shared.noto", "export fn base(): Int = 0\n"),
    ]);
    assert!(!sink.has_errors(), "{:?}", messages(&sink));
    assert_eq!(program.expect("loads").modules.len(), 4);
}

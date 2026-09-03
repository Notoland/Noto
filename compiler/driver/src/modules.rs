//! Finding the files a program is made of.
//!
//! A module is a file and its name is its path: `import geometry.point`
//! reads `geometry/point.noto` relative to the root file's directory. See
//! `docs/design/modules.md` for why there is no manifest and no search path.
//!
//! Loading happens before anything is checked. The root is parsed, its
//! imports are read, and so on until nothing is left — so a missing file or
//! an import cycle is reported once, with a span, rather than surfacing
//! halfway through type checking.

use noto_ast::{ItemKind, Module};
use noto_diagnostics::{codes, Diagnostic, DiagnosticSink};
use noto_semantic::{Import, ModuleId};
use noto_span::{FileId, Span};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One loaded module.
pub struct LoadedModule {
    /// The dotted name, empty for the root.
    pub name: String,
    /// The file it was read from.
    pub file: FileId,
    /// Its parsed contents.
    pub ast: Module,
    /// What it imports, in declaration order.
    pub imports: Vec<Import>,
}

/// Every module of one program, root first.
pub struct Program {
    /// The modules, indexed by [`ModuleId`].
    pub modules: Vec<LoadedModule>,
}

impl Program {
    /// Looks a module up.
    pub fn module(&self, id: ModuleId) -> &LoadedModule {
        &self.modules[id.0 as usize]
    }
}

/// Reads and parses the root file and everything it imports.
///
/// Returns `None` only when the root itself cannot be read; a module that is
/// missing further down is reported and the rest of the program is still
/// loaded, so one run names every broken import rather than the first.
pub fn load(
    map: &mut noto_span::SourceMap,
    root: &Path,
    sink: &mut DiagnosticSink,
) -> Option<Program> {
    let directory = root.parent().unwrap_or(Path::new(".")).to_path_buf();
    let file = crate::read_source(map, root, sink)?;

    let mut program = Program { modules: Vec::new() };
    let mut by_name: HashMap<String, ModuleId> = HashMap::new();
    // Node ids run across the whole program: analysis keys everything it
    // learns by id in one table, so two modules must not share one.
    let mut next_id = noto_ast::NodeId(0);

    let (ast, after) =
        noto_parser::parse_file_from(map.file(file).expect("just read"), next_id, sink);
    next_id = after;
    program.modules.push(LoadedModule {
        name: String::new(),
        file,
        ast,
        imports: Vec::new(),
    });

    // Breadth-first: every module's imports are resolved before the next
    // level, so the queue is also the order modules are reported in.
    let mut next = 0;
    while next < program.modules.len() {
        let id = ModuleId(next as u32);
        let imports = collect_imports(&program.modules[next].ast);
        next += 1;

        let mut resolved = Vec::new();
        for request in imports {
            let Some(target) = resolve(
                map,
                &directory,
                &request,
                &mut program,
                &mut by_name,
                &mut next_id,
                sink,
            ) else {
                continue;
            };
            resolved.push(Import {
                module: target,
                path: request.path,
                binding: request.binding,
                names: request.names,
                span: request.span,
            });
        }
        program.modules[id.0 as usize].imports = resolved;
    }

    report_cycles(&program, sink);
    Some(program)
}

/// An import as written, before its file is found.
struct Request {
    path: String,
    binding: Option<String>,
    names: Vec<noto_ast::Ident>,
    span: Span,
}

fn collect_imports(module: &Module) -> Vec<Request> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let ItemKind::Import(import) = &item.kind else { return None };
            // A selective import binds the names it lists and no namespace;
            // otherwise the namespace is the alias, or the path's last
            // segment.
            let binding = if import.names.is_empty() {
                Some(match &import.alias {
                    Some(alias) => alias.name.clone(),
                    None => import.path.last().name.clone(),
                })
            } else {
                None
            };
            Some(Request {
                path: import.path.to_dotted(),
                binding,
                names: import.names.clone(),
                span: item.span,
            })
        })
        .collect()
}

/// Finds or loads the module an import names.
fn resolve(
    map: &mut noto_span::SourceMap,
    directory: &Path,
    request: &Request,
    program: &mut Program,
    by_name: &mut HashMap<String, ModuleId>,
    next_id: &mut noto_ast::NodeId,
    sink: &mut DiagnosticSink,
) -> Option<ModuleId> {
    if let Some(id) = by_name.get(&request.path) {
        return Some(*id);
    }

    let path = module_path(directory, &request.path);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            sink.emit(
                Diagnostic::error(
                    codes::CANNOT_READ_FILE,
                    format!("cannot find module `{}`", request.path),
                )
                .with_primary(request.span, "imported here")
                .with_note(format!("expected it in `{}`", path.display()))
                .with_note(error.to_string()),
            );
            return None;
        }
    };

    let file = map.add(path.display().to_string(), text);
    let (ast, after) =
        noto_parser::parse_file_from(map.file(file).expect("just added"), *next_id, sink);
    *next_id = after;
    let id = ModuleId(program.modules.len() as u32);
    program.modules.push(LoadedModule {
        name: request.path.clone(),
        file,
        ast,
        imports: Vec::new(),
    });
    by_name.insert(request.path.clone(), id);
    Some(id)
}

/// `geometry.point` under `src/` is `src/geometry/point.noto`.
fn module_path(directory: &Path, name: &str) -> PathBuf {
    let mut path = directory.to_path_buf();
    for segment in name.split('.') {
        path.push(segment);
    }
    path.set_extension(crate::SOURCE_EXTENSION);
    path
}

/// Reports every import cycle, naming the modules that form it.
///
/// A cycle would mean deciding what a module sees of a half-initialised one,
/// so it is refused rather than ordered.
fn report_cycles(program: &Program, sink: &mut DiagnosticSink) {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        InProgress,
        Done,
    }

    let mut state = vec![State::Unvisited; program.modules.len()];
    let mut stack: Vec<ModuleId> = Vec::new();

    fn walk(
        id: ModuleId,
        program: &Program,
        state: &mut [State],
        stack: &mut Vec<ModuleId>,
        sink: &mut DiagnosticSink,
    ) {
        state[id.0 as usize] = State::InProgress;
        stack.push(id);

        for import in &program.module(id).imports {
            match state[import.module.0 as usize] {
                State::Done => {}
                State::Unvisited => walk(import.module, program, state, stack, sink),
                State::InProgress => {
                    let start = stack.iter().position(|m| *m == import.module).unwrap_or(0);
                    let names: Vec<String> = stack[start..]
                        .iter()
                        .map(|m| display_name(program.module(*m)))
                        .collect();
                    sink.emit(
                        Diagnostic::error(
                            codes::IMPORT_CYCLE,
                            format!("`{}` is imported in a cycle", import.path),
                        )
                        .with_primary(import.span, "this import closes the cycle")
                        .with_note(format!(
                            "{} -> {}",
                            names.join(" -> "),
                            display_name(program.module(import.module))
                        ))
                        .with_help("move what both modules need into a third module"),
                    );
                }
            }
        }

        stack.pop();
        state[id.0 as usize] = State::Done;
    }

    walk(ModuleId::ROOT, program, &mut state, &mut stack, sink);
}

/// What to call a module in a message; the root has no name of its own.
fn display_name(module: &LoadedModule) -> String {
    if module.name.is_empty() {
        "the root module".to_string()
    } else {
        module.name.clone()
    }
}

#[cfg(test)]
mod tests;

//! Builds a `.noto` file and writes the executable, for manual smoke testing.
use noto_diagnostics::{DiagnosticSink, RenderStyle};
use noto_driver::{compile, read_source, CompileOptions};
use noto_span::SourceMap;
use std::os::unix::fs::PermissionsExt;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: emit <input.noto> <output>");
    let output = args.next().expect("usage: emit <input.noto> <output>");

    let mut map = SourceMap::new();
    let mut sink = DiagnosticSink::new();
    let Some(file) = read_source(&mut map, std::path::Path::new(&input), &mut sink) else {
        print!("{}", sink.render_all(&map, RenderStyle::Plain));
        std::process::exit(1);
    };
    let result = compile(&map, file, &CompileOptions::default(), &mut sink);
    print!("{}", sink.render_all(&map, RenderStyle::Plain));
    match result.executable {
        Some(bytes) => {
            std::fs::write(&output, bytes).unwrap();
            let mut perms = std::fs::metadata(&output).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&output, perms).unwrap();
        }
        None => std::process::exit(1),
    }
}

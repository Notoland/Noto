# Diagnostics design

Diagnostics are part of the language surface. Every message has a stable
`NOTOnnnn` code, a primary span pointing at the right bytes, and a `help:`
line whenever there is a concrete fix. The codes are allocated in
`compiler/diagnostics/src/lib.rs`, module `codes` — **this document and that
module must agree.**

## The rules

- **Phases never print.** A phase pushes `Diagnostic`s into a
  `DiagnosticSink` and returns a best-effort result. The driver renders
  (plain for pipes and files, ANSI for a terminal) and decides the exit
  code. This is the seam the LSP will plug into.
- **One mistake, one diagnostic.** The error type absorbs everything — a
  type error does not cascade into five follow-up errors. There is a test
  holding this in place.
- **Codes are stable once shipped.** `NOTO0409` means "no implicit
  conversion" forever; tooling may match on codes.

## Code ranges

| Range | Phase | Examples |
|---|---|---|
| `0001`–`0003` | driver, file level | `CANNOT_READ_FILE`, `NO_MAIN`, `BAD_EXTENSION` |
| `01xx` | lexer | `UNTERMINATED_STRING`, `INVALID_ESCAPE`, `INVALID_NUMBER`, `UNTERMINATED_INTERPOLATION` |
| `02xx` | parser | `UNEXPECTED_TOKEN`, `INVALID_MODIFIER`, `MALFORMED_WHEN` |
| `03xx` | semantic, names | `UNKNOWN_NAME`, `DUPLICATE_NAME`, `REASSIGNED_VAL`, `OUTSIDE_LOOP`, `USED_BEFORE_INIT` |
| `04xx` | semantic, types | `TYPE_MISMATCH`, `ARITY_MISMATCH`, `NULLABLE_NOT_ALLOWED`, `NON_EXHAUSTIVE_WHEN`, `NO_IMPLICIT_CONVERSION`, `NON_BOOL_CONDITION` |
| `05xx` | lowering / backend | `UNSUPPORTED_CONSTRUCT` (not implemented in Noto 0.1), `UNSUPPORTED_TARGET`, `CANNOT_WRITE_OUTPUT` |
| `06xx` | linter | `UNUSED_BINDING`, `VAR_NEVER_REASSIGNED`, `UNREACHABLE_CODE`, `UNUSED_IMPORT` |

A new diagnostic takes the next free code in its phase's range; a new phase
gets a new range documented here and in `codes`.

## Rendering

```
error[NOTO0409]: `+` cannot mix `Int64` and `Int32`
 --> main.noto:4:17
  |
4 |     val c = a + b
  |                 ^ this is a `Int32`
  |
  = note: Noto never converts between number types on its own
  = help: convert it with `.toInt64()`
```

Level, code, message, the line with the primary span under it, then notes
and help. The driver prints a one-line summary at the end
(`2 errors, 1 warning`) and exits 1 when any error was reported.

## Recovery

Lexing and parsing continue after errors: the parser emits a diagnostic,
builds a best-effort node, and always consumes at least one token (every
recovery loop has a progress guard, and a test asserts parsing terminates
on arbitrary garbage). Semantic analysis runs only if parsing produced no
errors; lowering only if analysis is clean.

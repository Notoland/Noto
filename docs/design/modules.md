# Modules

Noto 0.4 introduced programs made of many files. This is what a module is,
how one finds another, and what crosses the boundary.

## A module is a file

One file, one module. Nothing declares a module name; the file's path is its
name.

The **root** is the file handed to the compiler, and its directory is the
root directory. Every other module is found from there: `import
geometry.point` reads `geometry/point.noto` relative to the root directory,
`import util` reads `util.noto`.

```
src/
  main.noto        the root, passed to `noto build`
  util.noto        module `util`
  geometry/
    point.noto     module `geometry.point`
```

No manifest, no search path, no build file. A package manifest will need to
say where the root is and where dependencies live, but that is a package
manager's job (RFC, later) and nothing about it changes what a module is.

`main` must be in the root module. So must the `test` declarations `noto
test` runs: testing a library means passing that library's file as the root,
which is what makes `noto test geometry/point.noto` mean what it looks like.

## `export` is the boundary

Every declaration is private to its module. `export` makes one visible to a
module that imports it:

```noto
export fn distance(a: Int, b: Int): Int = ...

fn helper(n: Int): Int = ...      // private to this module
```

Private by default is the choice that costs nothing to change later:
widening what a module exposes never breaks anyone, narrowing it does.

`export` applies to `fn`, `class` and `const`. A class's fields and methods
follow the class — exporting `Point` exports its constructor, its fields and
its methods. Finer visibility (`public`, `internal`, `protected`, which the
grammar already parses) waits until there is a package boundary for it to
mean something.

## `import` binds a name

```noto
import geometry.point                 // binds `point`
import geometry.point as geo          // binds `geo`
import geometry.point { distance }    // binds `distance`
```

A plain `import` binds the module's **last segment** as a namespace, and its
exports are reached through it: `point.distance(1, 2)`, `point.Point`. An
alias renames that namespace. A selective import binds the named exports
directly and binds no namespace.

There is no wildcard import. `import x.y` bringing everything into scope
unqualified is the feature every language that has it regrets: it makes the
origin of a name unknowable without reading every import, and it makes adding
an export to a library a breaking change.

An import that binds a name something else in the module already declares is
an error, and so is importing a name a module does not export — with the
exported names listed, because the usual cause is a missing `export`.

## The graph

Imports are resolved before anything is checked. The driver reads the root,
finds its imports, reads those, and repeats until nothing is left, then hands
every module to analysis at once.

**Cycles are an error.** `a` importing `b` importing `a` is reported with the
cycle spelled out, at the import that closes it. Allowing cycles would mean
deciding what a module sees of a half-initialised one, and there is nothing
to gain from it that splitting a module does not also give.

Every module is parsed and analysed, and the whole program is checked before
any code is generated. A type error in an imported module is reported against
that module's source, with the same spans and the same renderer.

## Name resolution, in order

For a name written unqualified, in one module:

1. locals in scope, innermost first;
2. that module's own top-level declarations;
3. names bound by a selective import;
4. builtins.

A module's own declaration wins over an imported one. Two selective imports
binding the same name is an error rather than a silent shadow.

For `namespace.name`, only the module that namespace binds is searched, and
only its exports.

## What this does not do yet

- No package manager, no external dependencies, no versions.
- No re-export: a module cannot pass on what it imported.
- No circular imports, as above.
- No visibility between `export` and private.
- `std/` is a directory of ordinary modules found the same way as any other,
  not a special path — until there is a package manager, a program that
  wants it copies it in.

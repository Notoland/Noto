# Generics

```noto
fn first<T>(xs: [T]): T {
    assert(xs.length > 0)
    return xs[0]
}

fn mapped<T, U>(xs: [T], f: fn(T): U): [U] {
    val out: [U] = []
    for x in xs { out.push(f(x)) }
    return out
}
```

## A generic function is compiled once

Not monomorphised — **erased**. `first<Int>` and `first<String>` are the same
machine code.

That is sound here for one reason, and it is worth stating plainly because it
is not a general truth: **every value in Noto is exactly one machine word.**
`Int` is 64 bits on every target by an earlier decision; `Int8` and `Int32`
are normalised into a word when loaded; a `String`, a list, an object, an
enum with data and a closure are all pointers; a `Bool` and a `Char` are
words holding small numbers. A generic function that only moves a `T` around
— stores it, passes it, returns it, puts it in a list — is moving a word, and
one copy of that code works for every `T`.

The alternative would have been monomorphisation: one compiled copy per set
of type arguments. It costs code size and it costs the ability to compile a
generic function without knowing its uses. It buys nothing while every value
is a word.

## What a type parameter permits

A `T` has no members, no operators and no literals. You may bind it, pass it,
return it, store it in a list or an object, and compare nothing about it.
That falls out of the type checker rather than being a rule of its own: `T`
is not `Int`, so `+` does not apply; it declares no fields, so `.x` does not
resolve.

This is deliberate. Bounds — `fn largest<T: Comparable>(..)` — are what would
lift it, and they need interfaces, which do not exist yet. Until then a
generic function is a plumbing function, and that covers most of what a
collection library is.

## Type arguments are inferred

There is no `first<Int>(xs)` syntax. The type arguments come from the
argument types, by matching the declared parameter types against them:
`[T]` against `[Int]` gives `T = Int`, `fn(T): U` against `fn(Int): String`
gives both. A parameter that appears in no argument cannot be inferred and is
an error at the call, naming which one.

Explicit type arguments parse today and are rejected. They will be needed
once a type parameter can appear only in the result.

## What would force this decision to be revisited

**Floating point.** A `Float64` lives in an SSE register and is passed in a
different register class, so a `T` holding one could not share code with a
`T` holding an `Int`. Landing floats means either monomorphising generic
functions, or boxing floats, or splitting the two register classes at the
call boundary. Whoever lands floats owns that choice, and this file is where
the reasoning it has to overturn is written down.

The same is true of any value wider than a word — a 128-bit integer, a
struct passed by value, a fat pointer.

## Not implemented

- Generic classes and enums: `class Box<T>(val value: T)`.
- Bounds of any kind.
- Explicit type arguments.
- A type parameter that appears only in the result type.

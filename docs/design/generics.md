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

## Classes are generic the same way

```noto
class Pair<A, B>(val first: A, val second: B) {
    fn swapped(): Pair<B, A> = Pair(second, first)
}
```

A class's type parameters are in scope for its fields, its properties and its
methods, and `this` inside one is a `Pair<A, B>` — the class applied to its
own parameters. Reading a field of a `Pair<Int, String>` substitutes them:
the field is declared `A` and comes out an `Int`.

Applied types are invariant: a `Box<Int>` is not a `Box<Any>`, for the reason
a `[Int]` is not a `[Any]`.

Erasure means one layout for every instantiation, which is already true of
every object: a field is a word, whatever it holds.

## Type arguments are inferred

There is no `first<Int>(xs)` syntax. The type arguments come from the
argument types, by matching the declared parameter types against them:
`[T]` against `[Int]` gives `T = Int`, `fn(T): U` against `fn(Int): String`
gives both.

What is expected of the call fills in the rest, which is the only way a
parameter appearing just in the result can be known:

```noto
fn emptyStack<T>(): Stack<T> { .. }
val numbers: Stack<Int> = emptyStack()
```

A parameter that neither an argument nor the expected type mentions is an
error at the call, naming which one. Explicit type arguments parse and are
rejected; they would be the third way to say it and nothing needs them yet.

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

- Generic enums: `enum Maybe<T> { Nothing, Just(value: T) }`.
- Bounds of any kind.
- Explicit type arguments.
- A type parameter that appears only in the result type.

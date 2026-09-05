# RFC 0003: Interfaces and bounds

- **Status:** Partially implemented — declarations and conformance are in;
  bounds and witnesses are not. See [Implementation status](#implementation-status).
- **Discussion:** (open a PR to discuss)

## Summary

Add `interface` declarations and type-parameter bounds (`fn largest<T: Comparable>(..)`)
to Noto. An interface names a set of methods and properties a type can promise
to provide; a bound is that promise attached to a type parameter, and it is
the only thing that lets a generic function do more with a `T` than move it
around. This RFC picks **nominal conformance declared at the type**, keeps
generics **erased** by threading a **witness pointer** per bound, and
deliberately does **not** yet make an interface a type a value can have.

## Motivation

A `T` today has no members, no operators and no literals — `docs/design/generics.md`
spells out why, and ends on the sentence this RFC exists to answer:

> Bounds — `fn largest<T: Comparable>(..)` — are what would lift it, and they
> need interfaces, which do not exist yet.

The concrete thing nobody can write:

```noto
// A dictionary. Blocked: comparing two keys needs `==` on a `K`,
// and a bare `K` permits only moving the value.
class Map<K, V> {
    fn get(key: K): V? { .. }   // needs key == storedKey
}

// Sorting. Blocked: same reason.
fn sorted<T>(xs: [T]): [T] { .. }   // needs a < b

// Structural equality for `data class`. Blocked: "two values are equal"
// is an interface a class implements, and there are no interfaces.
```

Every one of these is in the handoff's "what remains" list, and all of them
are the same missing feature. The grammar is already there: the parser reads
`interface Name<T> : Base { .. }`, `class C : I1, I2 { .. }` and
`<T: Bound + Other>` and `noto-semantic` rejects each with `NOTO0500`. The
grammar was settled ahead of the semantics on purpose. This RFC settles the
semantics.

## Proposal

### Declaring an interface

```noto
interface Comparable {
    fn compareTo(other: Self): Int

    // A default: an implementer gets this unless it overrides it.
    fn lessThan(other: Self): Bool = this.compareTo(other) < 0
}

interface Hashable {
    fn hash(): Int
}

// An interface may require a property, not just a method.
interface Sized {
    val size: Int
}

// An interface may extend others. Implementing `Ordered` requires
// implementing `Comparable` too.
interface Ordered : Comparable {
    fn min(other: Self): Self = if this.lessThan(other) { this } else { other }
}
```

`Self` is the type of the receiver — the implementing type, not the interface.
Inside `Comparable`, `other: Self` means "another value of whatever type
implements this", so a `Version` can only be compared to a `Version`, never to
a `Circle`. `Self` is new; it is a type expression legal only inside an
interface body and in the signature of a method that implements one.

An interface body may contain method signatures (no body — abstract),
methods with a body (a default), and `val`/`var` property requirements. It
may not contain stored fields, an `init`, or a constructor: an interface has
no storage and is never instantiated.

### Implementing an interface

Conformance is **nominal** and declared **where the type is declared**, in the
supertype list already parsed:

```noto
class Version(val major: Int, val minor: Int) : Comparable, Hashable {
    fn compareTo(other: Version): Int =
        if major != other.major { major - other.major }
        else { minor - other.minor }

    fn hash(): Int = major * 31 + minor
}
```

The checker verifies, at the class, that every abstract member of every
listed interface is present with a matching signature once `Self` is read as
the implementing type. A missing method is `NOTO0411`; a present method with
the wrong signature is `NOTO0412`, showing both signatures.

An `enum` may implement an interface the same way. A type implements a given
interface at most once — nominal conformance at a single site makes overlap
impossible, so there are no coherence rules to write.

### Built-in conformances

The user cannot open `class Int`, so the standard library declares a fixed,
compiler-known table of conformances for the primitive types:

| Type | Implements |
|---|---|
| `Int`, the sized ints, `Byte`, `Char` | `Comparable`, `Hashable` |
| `String` | `Comparable`, `Hashable` |
| `Bool` | `Hashable` |

These are facts the checker knows by name. `<T: Comparable>` is useful on day
one because `Int` and `String` satisfy it. Nothing else about primitives
changes: `a < b` on two `Int`s stays the builtin comparison it is today; the
interface table is consulted only when an `Int` flows into a bounded `T`.

### Using a bound

```noto
fn largest<T: Comparable>(xs: [T]): T {
    var best = xs[0]
    for x in xs {
        if best.lessThan(x) { best = x }
    }
    return best
}

fn main() {
    println(largest([3, 1, 4, 1, 5, 9]))              // T = Int
    println(largest(["pear", "apple", "quince"]))     // T = String
}
```

Inside `largest`, `best` and `x` have type `T`, and `T: Comparable` is what
makes `best.lessThan(x)` resolve — to `Comparable`'s default body, since `Int`
does not override `lessThan`. A call to a method the bound does not name is
still `NOTO0404`, "unknown member", exactly as an unbounded `T` gets today.

A bound also flows through construction:

```noto
class SortedList<T: Comparable> {
    var items: [T] = []
    fn insert(x: T) { /* uses x.compareTo(..) to find the slot */ }
}
```

A generic class with a bounded parameter may call the bound's methods in its
own methods. `SortedList<Version>()` at a call site checks that `Version:
Comparable` and is otherwise ordinary.

### A bound is checked at every call

```noto
class Circle(val r: Int)

largest([Circle(1), Circle(2)])
//      ^^^^^^^^^^^^^^^^^^^^^^^ NOTO0413: `Circle` is not `Comparable`
//      help: declare `class Circle(..) : Comparable` and give it `compareTo`
```

### Interfaces are not yet value types

This RFC does **not** let an interface stand where a type is expected outside
a bound:

```noto
val c: Comparable = Version(1, 0)
//     ^^^^^^^^^^ NOTO0414: an interface is not a value type
//     help: take it as a bounded type parameter — `fn f<T: Comparable>(x: T)`

val things: [Drawable] = [circle, square]   // same error
```

Heterogeneous collections and dynamic dispatch through an interface are a real
feature and a separable one. They are the subject of a follow-up RFC, because
they force a value to be two words (a data pointer plus a witness pointer),
and "every value is one machine word" is load-bearing enough — see
`docs/design/generics.md` — that widening it deserves its own decision with
its own consensus. Bounds unblock `Map`, `sorted`, `Hashable` and `data class`
without touching the value model, so they go first.

## Semantics

### Conformance checking

For `class C<...> : I` where `I` resolves to an interface:

1. Substitute `C<...>` for `Self` throughout `I`'s member signatures, and
   substitute `I`'s type arguments for its parameters.
2. For each abstract method of `I` (and, transitively, of every interface `I`
   extends), require a method on `C` with the same name, the same parameter
   types, the same result type, and the same nullability. Default methods are
   satisfied automatically and may be overridden.
3. For each property requirement, require a field or property on `C` of a
   matching type; a `var` requirement is not met by a `val`.
4. Record `(C, I) -> witness` in the conformance table (see below).

Conformance is checked once, at the declaration, independent of any use.

### Erasure survives — the witness

A generic function stays **compiled once**. A bound does not change that; it
adds a hidden parameter.

`fn largest<T: Comparable>(xs: [T]): T` compiles to a function of **two**
machine arguments: `xs`, and a pointer to the **witness** for `T:
Comparable` — a small static table of the concrete type's implementations of
`Comparable`'s methods, in a fixed order:

```
witness Int : Comparable = { compareTo: Int.compareTo, lessThan: Comparable.lessThan$default }
```

At a call, the compiler knows the concrete `T`, looks up `(T, Comparable)` in
the conformance table, and passes that witness. `best.lessThan(x)` inside the
body is a load of the `lessThan` slot from the witness followed by an indirect
call with `best` as the receiver.

- **One witness pointer per bound per call.** `<T: A + B>` passes two;
  `<T: A, U: B>` passes two. This is the first argument in the language that
  is not part of the source-level signature, and the handoff predicts it
  exactly: *"a bounded `T` may need to carry a witness, and that is a pointer
  per call, which is the first thing in the language that would not be free."*
- **Unbounded generics are byte-for-byte unchanged.** `fn first<T>(xs: [T])`
  passes no witness and compiles as it does today.
- **The witness is static data.** One table per `(type, interface)` pair
  actually used, in the read-only image. No allocation, no runtime
  construction for the function case.

### Bounds on a generic class

A class with a bounded parameter stores the witness as a hidden field, set at
construction from the call-site witness:

```
SortedList<T: Comparable>  layout:  [ items: ptr ][ T$Comparable: ptr ]
```

`SortedList<Version>()` writes the `Version : Comparable` witness into the
hidden slot; `insert` reads it to call `compareTo`. One hidden word per
bounded parameter — consistent with "a field is a word".

### Interface extension and witness composition

`interface Ordered : Comparable` — the `Ordered` witness embeds (or points to)
the `Comparable` witness, so a function bounded `<T: Ordered>` can call
`Comparable` methods through it. Implementing `Ordered` on a class requires
`Comparable` to be listed too; the checker does not silently synthesise it.

### Interaction with existing features

- **`when` / `is` / `as`:** unchanged by this RFC. `x is Comparable` is not
  expressible because `Comparable` is not a value type here. When interface
  value types land, that is where narrowing to an interface gets defined.
- **Nullability:** `T: Comparable` says nothing about `T?`. A `T?` still needs
  a null check before any member access, bound or not.
- **`==`:** stays a builtin on primitives and stays unavailable on a bare `T`.
  Whether `data class` equality is `Eq` (an interface a class implements or
  derives) is left to RFC 0001's `data class` follow-up; this RFC only makes
  such an interface expressible.
- **`defer`, lambdas, lists:** no interaction.
- **Formatter / linter:** an interface body formats as a class body does; a
  lint for an unused type parameter extends naturally to an unused bound.

### Diagnostics this adds

| Code | Meaning |
|---|---|
| `NOTO0411` | a type is missing a method its interface requires |
| `NOTO0412` | a method meant to implement an interface has the wrong signature |
| `NOTO0413` | a type argument does not satisfy a bound at a call or constructor |
| `NOTO0414` | an interface used where a value type is expected |
| `NOTO0415` | `interface` body has a stored field, `init` or constructor |
| `NOTO0416` | `Self` used outside an interface or an implementing method |

All are errors, each with a `help:` naming the concrete fix.

## Alternatives considered

### Structural conformance

A type satisfies `Comparable` because it happens to have a `compareTo(Self):
Int`, no declaration needed. Go's interfaces work this way.

Rejected. Every other kind in Noto is nominal — an enum case is its tag, an
object is a reference of a named class, `is`/`as` test named types. Structural
conformance would be the one place the language infers a relationship the
programmer did not write, and "does `Widget` implement `Drawable`?" would stop
being answerable by reading `Widget`'s first line. It also makes an accidental
signature match a silent semantic commitment.

### Monomorphise bounded generics

Keep unbounded generics erased, but compile one copy of `largest` per concrete
`T`, resolving `compareTo` statically in each. No witness pointer.

Rejected. It splits generics into two mental and implementation models
(erased here, monomorphised there) along a line — "does it have a bound" —
that is invisible at the call site. It loses the property that a generic
function can be compiled without seeing its uses. And it trades a predictable
one-pointer cost for an unpredictable code-size one. Witness passing keeps one
model: everything is erased, a bound is just a hidden argument.

### Universal boxing with runtime type info

Every value carries a type tag; interface dispatch is a runtime lookup from
tag to method table. No per-call pointer, no monomorphisation.

Rejected hardest. It puts a tag on every value in the language to serve the
minority that flow through a bound, breaking "every value is one machine word"
everywhere to avoid breaking it in one place.

### `impl Comparable for Version` blocks, Rust-style, with an orphan rule

Allow conformance to be declared away from the type, so you could implement
your interface for a type from another module.

Deferred, not rejected. It is strictly more expressive — retroactive
conformance is genuinely useful — but it needs coherence rules (who may
write the impl, what happens when two modules both do), and it means a type's
interface set is no longer readable at the type. The built-in table covers the
one case that truly cannot wait (primitives), and declaration-site conformance
covers user types. This can be added later without breaking anything the
narrow rule allowed.

## Drawbacks and costs

- **A new hidden argument.** Bounded generic calls pass one pointer per bound.
  Measurable, documented, and paid only by code that uses a bound — but it is
  the first crack in "the signature you read is the signature that is called".
- **`Self` is new surface.** One more type-level concept to learn, though it
  only appears in interface definitions and the methods that satisfy them.
- **Compiler complexity.** A conformance table, witness tables in the object
  file, witness threading through lowering and codegen, `Self` resolution,
  and the substitution logic in the checker. Roughly the size of the generic
  classes work, per the handoff's own estimate.
- **Binary size.** One witness table per `(type, interface)` pair used. Small,
  but non-zero, and it grows with the standard library's interface use.
- **A visible gap.** Interfaces that are not value types will feel
  half-finished to anyone reaching for `[Drawable]`. The follow-up RFC is the
  answer, and the `NOTO0414` help text points at the bound form meanwhile.
- **Teaching.** "Generics are free" becomes "generics are free unless bounded,
  and then it is one pointer per bound" — a caveat where there was none.

## Decided

These were open when this RFC was written and are now settled. Each is
enforced by a test.

2. **Generic interfaces: out of scope.** `interface Into<T>` is rejected with
   `NOTO0500`. Allowing it turns "a type implements an interface at most once"
   into "at most once per type argument tuple", which brings back exactly the
   coherence questions this RFC was written to avoid. It can be added later
   without invalidating anything the narrow rule allowed.

3. **Operators do not desugar.** `a < b` on a bounded `T: Comparable` is *not*
   `a.compareTo(b) < 0`. You write `a.lessThan(b)`. Noto has been strict about
   an operator meaning one thing — no implicit numeric conversion, bitwise
   binding tighter than comparison — and making `<` mean a method call when
   and only when a bound is in scope would put that meaning somewhere the
   reader of the call cannot see. Loosening this later is possible; tightening
   it back would not be.

6. **A default method may read a property the interface requires.** That is
   most of what defaults are for. The consequence is that a witness carries
   property accessors as well as method pointers.

7. **`Self` is legal in parameter and result position.** A default body may
   not construct a `Self`: an interface cannot name the implementer's
   constructor.

## Still unresolved

Each needs an answer before this RFC is `Implemented`.

1. **Witness representation.** A table of function pointers (simple, one
   indirection per call) versus specialising the common single-method case
   into a bare function pointer. Start with the table; measure.
4. **Standard interfaces the compiler knows by name.** `Eq`, `Ord`, `Hashable`
   — does the compiler special-case them (for `==`, for `when`, for `data
   class` derivation), or are they ordinary library interfaces? This decides
   how `data class` gets its equality.
5. **Bounds referencing the enclosing parameters.** `<T, U: Container<T>>` —
   allowed? It is the point where bound resolution needs its own fixpoint.
   In scope for the bounds work, not deferred, but not yet designed.
8. **Interaction with the memory model (RFC 0001).** A witness is static for
   the function case but a per-object field for the bounded-class case. If
   objects gain destructors, does a witness field participate? It should not —
   it points at static data — but that needs stating.

## Implementation status

Landed, checker only — `noto-semantic`, plus `DefKind::Interface` and the
diagnostic codes:

- `interface` declarations with abstract method and property requirements
- `Self`, as type parameter 0 of the interface's own def, so that reading it
  as the implementing type is one entry in the substitution the generics work
  already uses. `Self` anywhere else is `NOTO0416`
- nominal conformance checked once at the class, against every interface it
  lists, with `NOTO0411` for a missing member and `NOTO0412` for one whose
  signature does not match
- an interface that extends another must have that other listed at the
  implementing type too; nothing is synthesised
- `NOTO0414` for an interface named where a value type is expected, and
  `NOTO0415` for a body that tries to store something

This slice needs no lowering and emits no code: with abstract members only, an
interface declaration compiles to nothing and an implementing class is laid
out exactly as it was before. `examples/interfaces.noto` builds to a native
binary and runs.

Not yet landed:

- **bounds** (`<T: Comparable>`), and `NOTO0413` for a type argument that does
  not satisfy one. Still rejected with `NOTO0500`
- **witnesses** — no dispatch through a bound exists, so nothing calls an
  interface member except through the concrete type
- **default method bodies.** Rejected with `NOTO0500`: reaching one means
  dispatching through a witness. This is why `has_default` is not yet recorded
- **built-in conformances for the primitives** (`Int: Comparable`, ...)
- **interfaces on an enum.** Still rejected, because an enum cannot have
  methods at all yet

### A formatting note

The prose above writes `class Version(..) : Comparable`, with a space before
the colon. The formatter writes `class Version(..): Comparable`, because
`docs/design/formatter.md` says *never a space before `:`* and a supertype
list was never exercised before this landed. The formatter follows the rule
that is written down; these examples do not. Whether a supertype colon
deserves an exception is a formatter decision that nobody has made.

## What this RFC must not do

Decide, in passing, that Noto has runtime type information, dynamic dispatch
through interface values, or retroactive conformance. Each is a real feature
with its own trade-offs, and each is easier to add on top of a small, sound
core than to walk back out of a large one. This RFC is the small core:
nominal, declaration-site, erased, bounds-only.

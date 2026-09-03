//! Lexical scopes and the symbol table.

use crate::analysis::Resolution;
use std::collections::HashMap;

/// A stack of lexical scopes.
///
/// Lookup walks outwards from the innermost scope, so an inner declaration
/// shadows an outer one. Shadowing inside the *same* scope is reported by the
/// caller, which has the spans needed for a good message.
#[derive(Default)]
pub struct Scopes {
    scopes: Vec<Scope>,
}

#[derive(Default)]
struct Scope {
    names: HashMap<String, Resolution>,
    /// Whether the scope is a loop body, which is what `break` and `continue`
    /// need to know.
    is_loop: bool,
}

impl Scopes {
    /// Creates an empty stack.
    pub fn new() -> Self {
        Scopes { scopes: Vec::new() }
    }

    /// Enters a new scope.
    pub fn push(&mut self) {
        self.scopes.push(Scope::default());
    }

    /// Enters a new scope that is the body of a loop.
    pub fn push_loop(&mut self) {
        self.scopes.push(Scope { names: HashMap::new(), is_loop: true });
    }

    /// Leaves the innermost scope.
    pub fn pop(&mut self) {
        self.scopes.pop();
    }

    /// Leaves the innermost scope and returns what it bound.
    ///
    /// This is how a module's top-level names are kept: they are collected
    /// into a scope like any others, then lifted out to be re-entered when
    /// that module's bodies are checked.
    pub fn take_top(&mut self) -> HashMap<String, Resolution> {
        self.scopes.pop().map(|scope| scope.names).unwrap_or_default()
    }

    /// Binds a name in the innermost scope, returning what it displaced there.
    pub fn declare(&mut self, name: impl Into<String>, resolution: Resolution) -> Option<Resolution> {
        let scope = self.scopes.last_mut().expect("a scope must be open");
        scope.names.insert(name.into(), resolution)
    }

    /// Looks a name up, innermost scope first.
    pub fn lookup(&self, name: &str) -> Option<Resolution> {
        self.scopes.iter().rev().find_map(|scope| scope.names.get(name).copied())
    }

    /// Whether the name is bound in the innermost scope.
    pub fn is_declared_here(&self, name: &str) -> bool {
        self.scopes.last().is_some_and(|scope| scope.names.contains_key(name))
    }

    /// Whether any enclosing scope is a loop body.
    pub fn in_loop(&self) -> bool {
        self.scopes.iter().any(|scope| scope.is_loop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::LocalId;

    fn local(index: u32) -> Resolution {
        Resolution::Local(LocalId(index))
    }

    #[test]
    fn inner_scopes_shadow_outer_ones() {
        let mut scopes = Scopes::new();
        scopes.push();
        scopes.declare("x", local(0));
        scopes.push();
        scopes.declare("x", local(1));
        assert_eq!(scopes.lookup("x"), Some(local(1)));
        scopes.pop();
        assert_eq!(scopes.lookup("x"), Some(local(0)));
    }

    #[test]
    fn declaring_twice_in_one_scope_reports_the_previous_binding() {
        let mut scopes = Scopes::new();
        scopes.push();
        assert_eq!(scopes.declare("x", local(0)), None);
        assert_eq!(scopes.declare("x", local(1)), Some(local(0)));
        assert!(scopes.is_declared_here("x"));
    }

    #[test]
    fn loop_membership_is_visible_from_nested_scopes() {
        let mut scopes = Scopes::new();
        scopes.push();
        assert!(!scopes.in_loop());
        scopes.push_loop();
        scopes.push();
        assert!(scopes.in_loop());
        scopes.pop();
        scopes.pop();
        assert!(!scopes.in_loop());
    }
}

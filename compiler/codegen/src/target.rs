//! Target selection.

use std::fmt;

/// A processor architecture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Architecture {
    /// 64-bit x86.
    X86_64,
}

impl Architecture {
    /// The name used in a target triple.
    pub fn name(self) -> &'static str {
        match self {
            Architecture::X86_64 => "x86_64",
        }
    }

    /// Parses an architecture name.
    pub fn from_name(name: &str) -> Option<Architecture> {
        match name {
            "x86_64" | "amd64" => Some(Architecture::X86_64),
            _ => None,
        }
    }
}

/// An operating system.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum OperatingSystem {
    /// Linux, using raw syscalls and ELF executables.
    Linux,
}

impl OperatingSystem {
    /// The name used in a target triple.
    pub fn name(self) -> &'static str {
        match self {
            OperatingSystem::Linux => "linux",
        }
    }

    /// Parses an operating system name.
    pub fn from_name(name: &str) -> Option<OperatingSystem> {
        match name {
            "linux" => Some(OperatingSystem::Linux),
            _ => None,
        }
    }
}

/// What a compilation is producing code for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Target {
    /// The processor architecture.
    pub architecture: Architecture,
    /// The operating system.
    pub operating_system: OperatingSystem,
}

impl Target {
    /// Linux on x86-64: the first target Noto supports.
    pub const LINUX_X86_64: Target =
        Target { architecture: Architecture::X86_64, operating_system: OperatingSystem::Linux };

    /// Every target this compiler can generate code for.
    pub fn supported() -> &'static [Target] {
        &[Target::LINUX_X86_64]
    }

    /// Whether a backend exists for this target.
    pub fn is_supported(self) -> bool {
        Target::supported().contains(&self)
    }

    /// The target the compiler is running on.
    pub fn host() -> Target {
        Target::LINUX_X86_64
    }

    /// Parses a target triple such as `x86_64-linux`.
    pub fn parse(triple: &str) -> Option<Target> {
        let mut parts = triple.split('-');
        let architecture = Architecture::from_name(parts.next()?)?;
        let operating_system = OperatingSystem::from_name(parts.next()?)?;
        Some(Target { architecture, operating_system })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.architecture.name(), self.operating_system.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triples_round_trip() {
        let target = Target::LINUX_X86_64;
        assert_eq!(target.to_string(), "x86_64-linux");
        assert_eq!(Target::parse("x86_64-linux"), Some(target));
        assert_eq!(Target::parse("amd64-linux"), Some(target));
    }

    #[test]
    fn unknown_triples_are_rejected() {
        assert_eq!(Target::parse("riscv64-linux"), None);
        assert_eq!(Target::parse("x86_64-windows"), None);
        assert_eq!(Target::parse("x86_64"), None);
        assert_eq!(Target::parse(""), None);
    }

    #[test]
    fn the_host_target_has_a_backend() {
        assert!(Target::host().is_supported());
    }
}

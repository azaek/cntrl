//! Services behind the `service.*` operations: systemd units on Linux, through
//! [`crate::systemd`].

use crate::HostError;

/// Unit types other than services. `service.*` operations refuse them, so they
/// never touch targets, mounts or sockets.
const OTHER_TYPES: &[&str] = &[
    "socket",
    "target",
    "device",
    "mount",
    "automount",
    "swap",
    "timer",
    "path",
    "slice",
    "scope",
];

/// Checks a service's unit name, adding `.service` when it has no type, so
/// `nginx` names `nginx.service`.
pub fn service_unit(name: &str) -> Result<String, HostError> {
    let invalid = |why: &str| HostError::Invalid(format!("`{name}` isn't a service unit: {why}"));
    let unit = match name.rsplit_once('.') {
        Some((_, "service")) => name.to_owned(),
        Some((_, kind)) if OTHER_TYPES.contains(&kind) => return Err(invalid("not a service")),
        _ => format!("{name}.service"),
    };
    let prefix = unit.strip_suffix(".service").unwrap_or(&unit);
    if prefix.is_empty() || unit.len() > 255 {
        return Err(invalid("the name must have 1 to 247 characters"));
    }
    // systemd's alphabet for unit names (systemd.unit(5)).
    let allowed = |c: char| c.is_ascii_alphanumeric() || ":-_.\\@".contains(c);
    if !prefix.chars().all(allowed) {
        return Err(invalid("only letters, digits and `:-_.\\@` are allowed"));
    }
    if prefix.ends_with('@') {
        return Err(invalid("a template needs an instance, as in `getty@tty1`"));
    }
    Ok(unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_the_service_suffix() {
        assert_eq!(service_unit("nginx"), Ok("nginx.service".to_owned()));
        assert_eq!(
            service_unit("nginx.service"),
            Ok("nginx.service".to_owned())
        );
        assert_eq!(
            service_unit("getty@tty1"),
            Ok("getty@tty1.service".to_owned())
        );
        // A dot alone isn't a type.
        assert_eq!(service_unit("my.app"), Ok("my.app.service".to_owned()));
    }

    #[test]
    fn refuses_other_unit_types() {
        assert!(matches!(
            service_unit("multi-user.target"),
            Err(HostError::Invalid(_))
        ));
        assert!(matches!(
            service_unit("ssh.socket"),
            Err(HostError::Invalid(_))
        ));
    }

    #[test]
    fn refuses_malformed_names() {
        for name in [
            "",
            ".service",
            "a b",
            "../etc/passwd",
            "x;reboot",
            "getty@",
            "getty@.service",
        ] {
            assert!(
                matches!(service_unit(name), Err(HostError::Invalid(_))),
                "{name} should be refused"
            );
        }
        assert!(service_unit(&"a".repeat(248)).is_err());
        assert!(service_unit(&"a".repeat(247)).is_ok());
    }
}

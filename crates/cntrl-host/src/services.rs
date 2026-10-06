//! Services behind the `service.*` operations: systemd units on Linux, through
//! [`crate::systemd`], launchd jobs on macOS, through `crate::launchd`, and the
//! Service Control Manager's services on Windows, through
//! `crate::windows::services`.

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

/// Checks a service's name as this OS names services: a systemd unit on Linux
/// ([`service_unit`]), a launchd label on macOS ([`launchd_label`]), a
/// service's name on Windows ([`windows_service`]).
pub fn service_name(name: &str) -> Result<String, HostError> {
    #[cfg(target_os = "macos")]
    {
        launchd_label(name)
    }
    #[cfg(windows)]
    {
        windows_service(name)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        service_unit(name)
    }
}

/// Checks a Windows service's name, such as `Spooler` or `MSSQL$SQLEXPRESS`:
/// up to 256 characters, without slashes, which Windows refuses in a name.
/// Windows compares names without regard to case.
pub fn windows_service(name: &str) -> Result<String, HostError> {
    let invalid =
        |why: &str| HostError::Invalid(format!("`{name}` isn't a Windows service's name: {why}"));
    if name.is_empty() || name.chars().count() > 256 {
        return Err(invalid("the name must have 1 to 256 characters"));
    }
    if name
        .chars()
        .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Err(invalid("slashes aren't allowed"));
    }
    Ok(name.to_owned())
}

/// Checks a launchd job's label, such as `com.openssh.sshd`: letters, digits
/// and `.-_`. A `/` would name another launchd domain, so it's refused.
pub fn launchd_label(name: &str) -> Result<String, HostError> {
    let invalid = |why: &str| HostError::Invalid(format!("`{name}` isn't a launchd label: {why}"));
    if name.is_empty() || name.len() > 255 {
        return Err(invalid("the name must have 1 to 255 characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
    {
        return Err(invalid("only letters, digits and `.-_` are allowed"));
    }
    if name.starts_with(['.', '-']) {
        return Err(invalid("it can't start with `.` or `-`"));
    }
    Ok(name.to_owned())
}

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
    fn checks_launchd_labels() {
        assert_eq!(
            launchd_label("com.openssh.sshd"),
            Ok("com.openssh.sshd".to_owned())
        );
        assert_eq!(
            launchd_label("homebrew.mxcl.nginx"),
            Ok("homebrew.mxcl.nginx".to_owned())
        );
        for name in [
            "",
            "gui/501/x",
            "system/com.apple.foo",
            "a b",
            "-k",
            ".hidden",
            "x;reboot",
        ] {
            assert!(
                matches!(launchd_label(name), Err(HostError::Invalid(_))),
                "{name} should be refused"
            );
        }
    }

    #[test]
    fn checks_windows_service_names() {
        assert_eq!(
            windows_service("MSSQL$SQLEXPRESS").expect("valid"),
            "MSSQL$SQLEXPRESS"
        );
        assert_eq!(
            windows_service("cntrl-agent").expect("valid"),
            "cntrl-agent"
        );
        assert!(windows_service("").is_err());
        assert!(windows_service("a\\b").is_err());
        assert!(windows_service("a/b").is_err());
        assert!(windows_service(&"x".repeat(257)).is_err());
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

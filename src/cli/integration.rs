use crate::api::schema::IntegrationTarget;

pub(super) fn run_integration_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_integration_help();
        return Ok(2);
    };

    match subcommand {
        "install" => integration_install(&args[1..]),
        "uninstall" => integration_uninstall(&args[1..]),
        "status" => integration_status(&args[1..]),
        "help" | "--help" | "-h" => {
            print_integration_help();
            Ok(0)
        }
        _ => {
            print_integration_help();
            Ok(2)
        }
    }
}

fn integration_status(args: &[String]) -> std::io::Result<i32> {
    let outdated_only = match args {
        [] => false,
        [flag] if flag == "--outdated-only" => true,
        _ => {
            eprintln!("usage: herdr integration status [--outdated-only]");
            return Ok(2);
        }
    };

    if outdated_only {
        crate::integration::print_outdated_update_notice();
        return Ok(0);
    }

    for status in crate::integration::installed_integration_statuses() {
        let target = crate::integration::integration_target_label(status.target);
        let version = match status.installed_version {
            Some(version) => format!("v{version}"),
            None => "legacy".to_string(),
        };
        let state = match status.state {
            crate::integration::IntegrationStatusKind::NotInstalled => "not installed".to_string(),
            crate::integration::IntegrationStatusKind::Current => {
                format!("current ({version})")
            }
            crate::integration::IntegrationStatusKind::Outdated => {
                format!("outdated ({version} < v{})", status.expected_version)
            }
        };
        println!("{target}: {state} ({})", status.path.display());
    }

    Ok(0)
}

fn integration_install(args: &[String]) -> std::io::Result<i32> {
    let Some((target, extra)) = parse_integration_target(args, "install")? else {
        return Ok(2);
    };

    let result = if extra.is_empty() {
        crate::integration::install_target(target)
    } else {
        crate::integration::install_claude_extra_dirs(
            &extra
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        )
    };

    match result {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn integration_uninstall(args: &[String]) -> std::io::Result<i32> {
    let Some((target, extra)) = parse_integration_target(args, "uninstall")? else {
        return Ok(2);
    };

    let result = if extra.is_empty() {
        crate::integration::uninstall_target(target)
    } else {
        crate::integration::uninstall_claude_extra_dirs(
            &extra
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        )
    };

    match result {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn print_integration_messages(messages: Vec<String>) {
    for message in messages {
        println!("{message}");
    }
}

fn parse_integration_target(
    args: &[String],
    action: &str,
) -> std::io::Result<Option<(IntegrationTarget, Vec<std::path::PathBuf>)>> {
    let print_usage = || {
        eprintln!(
            "usage: herdr integration {action} <pi|omp|claude|codex|copilot|devin|droid|kimi|opencode|kilo|hermes|qodercli|cursor|mastracode> [--config-dir <path>]..."
        );
    };

    let Some(target_str) = args.first().map(|arg| arg.as_str()) else {
        print_usage();
        return Ok(None);
    };

    let parsed = match target_str {
        "pi" => IntegrationTarget::Pi,
        "omp" => IntegrationTarget::Omp,
        "claude" => IntegrationTarget::Claude,
        "codex" => IntegrationTarget::Codex,
        "copilot" => IntegrationTarget::Copilot,
        "devin" => IntegrationTarget::Devin,
        "droid" => IntegrationTarget::Droid,
        "kimi" => IntegrationTarget::Kimi,
        "opencode" => IntegrationTarget::Opencode,
        "kilo" => IntegrationTarget::Kilo,
        "hermes" => IntegrationTarget::Hermes,
        "qodercli" => IntegrationTarget::Qodercli,
        "cursor" => IntegrationTarget::Cursor,
        "mastracode" => IntegrationTarget::Mastracode,
        _ => {
            eprintln!("unknown integration target: {target_str}");
            eprintln!(
                "currently supported: pi, omp, claude, codex, copilot, devin, droid, kimi, opencode, kilo, hermes, qodercli, cursor, mastracode"
            );
            return Ok(None);
        }
    };

    let mut extra_dirs: Vec<std::path::PathBuf> = Vec::new();
    let remaining = &args[1..];
    let mut i = 0;
    while i < remaining.len() {
        match remaining[i].as_str() {
            "--config-dir" => {
                if parsed != IntegrationTarget::Claude {
                    eprintln!("--config-dir is only valid for the claude integration target");
                    return Ok(None);
                }
                i += 1;
                if i >= remaining.len() {
                    eprintln!("--config-dir requires a path argument");
                    print_usage();
                    return Ok(None);
                }
                extra_dirs.push(std::path::PathBuf::from(&remaining[i]));
            }
            _ => {
                print_usage();
                return Ok(None);
            }
        }
        i += 1;
    }

    Ok(Some((parsed, extra_dirs)))
}

fn print_integration_help() {
    eprintln!("herdr integration commands:");
    eprintln!("  herdr integration install pi");
    eprintln!("  herdr integration install omp");
    eprintln!("  herdr integration install claude");
    eprintln!("  herdr integration install codex");
    eprintln!("  herdr integration install copilot");
    eprintln!("  herdr integration install devin");
    eprintln!("  herdr integration install droid");
    eprintln!("  herdr integration install kimi");
    eprintln!("  herdr integration install opencode");
    eprintln!("  herdr integration install kilo");
    eprintln!("  herdr integration install hermes");
    eprintln!("  herdr integration install qodercli");
    eprintln!("  herdr integration install cursor");
    eprintln!("  herdr integration install mastracode");
    eprintln!("  herdr integration uninstall pi");
    eprintln!("  herdr integration uninstall omp");
    eprintln!("  herdr integration uninstall claude");
    eprintln!("  herdr integration uninstall codex");
    eprintln!("  herdr integration uninstall copilot");
    eprintln!("  herdr integration uninstall devin");
    eprintln!("  herdr integration uninstall droid");
    eprintln!("  herdr integration uninstall kimi");
    eprintln!("  herdr integration uninstall opencode");
    eprintln!("  herdr integration uninstall kilo");
    eprintln!("  herdr integration uninstall hermes");
    eprintln!("  herdr integration uninstall qodercli");
    eprintln!("  herdr integration uninstall cursor");
    eprintln!("  herdr integration uninstall mastracode");
    eprintln!("  herdr integration status [--outdated-only]");
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parse_target_collects_config_dir_flags() {
        let args = vec![
            "claude".to_string(),
            "--config-dir".to_string(),
            "/a".to_string(),
            "--config-dir".to_string(),
            "/b".to_string(),
        ];
        let parsed = parse_integration_target(&args, "install").unwrap().unwrap();
        assert_eq!(parsed.0, IntegrationTarget::Claude);
        assert_eq!(parsed.1, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}

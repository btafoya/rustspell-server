//! Offline administrative CLI for the Rust Spell Server.
//!
//! Currently exposes one subcommand: `reset-platform-key`, an escape hatch for
//! the case where the bootstrap `platform` key printed on first start has been
//! lost or compromised.

use std::io::{BufRead, IsTerminal, Write};

use crate::config;
use crate::store::Store;

/// Run the `reset-platform-key` command.
///
/// `args` contains the remaining arguments after the `reset-platform-key`
/// subcommand itself.
pub async fn run(args: &[String]) -> anyhow::Result<()> {
    let config = config::load()?;
    let opts = parse_args(args)?;

    if !opts.yes {
        require_terminal()?;
        confirm_interactive(&mut std::io::stdin().lock(), &mut std::io::stderr())?;
    }

    let store = Store::open_for_cli(&config).await?;
    let created = store.reset_bootstrap_platform_key().await?;

    print_output(&opts.format, &created.raw_key)?;

    if let Ok(path) = std::env::var("RUSTSPELL_BOOTSTRAP_SECRETS_PATH") {
        write_bootstrap_secrets(&path, &created.raw_key)?;
    }

    Ok(())
}

/// Write the freshly created bootstrap platform key to a JSON file so external
/// tooling (e.g., the live API test suite) can authenticate without scraping
/// stdout. Used both at startup and by the CLI reset command.
pub fn write_bootstrap_secrets(path: &str, platform_key: &str) -> anyhow::Result<()> {
    let secrets = serde_json::json!({ "platform_key": platform_key });
    let contents = serde_json::to_string_pretty(&secrets)?;
    std::fs::write(path, contents)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OutputFormat {
    #[default]
    Human,
    Json,
    Quiet,
}

#[derive(Debug, Default)]
struct Options {
    yes: bool,
    format: OutputFormat,
}

fn parse_args(args: &[String]) -> anyhow::Result<Options> {
    let mut opts = Options::default();

    for arg in args {
        match arg.as_str() {
            "--yes" => opts.yes = true,
            "--json" => {
                if opts.format != OutputFormat::Human {
                    anyhow::bail!("--json and --quiet are mutually exclusive");
                }
                opts.format = OutputFormat::Json;
            }
            "--quiet" => {
                if opts.format != OutputFormat::Human {
                    anyhow::bail!("--json and --quiet are mutually exclusive");
                }
                opts.format = OutputFormat::Quiet;
            }
            _ => anyhow::bail!("unknown argument: {arg}"),
        }
    }

    Ok(opts)
}

fn require_terminal() -> anyhow::Result<()> {
    if !std::io::stderr().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "not running in a terminal; use --yes to confirm the bootstrap platform key reset"
        );
    }
    Ok(())
}

fn confirm_interactive(stdin: &mut dyn BufRead, stderr: &mut dyn Write) -> anyhow::Result<()> {
    write!(
        stderr,
        "This will invalidate the existing bootstrap platform key and issue a new one. Continue? [y/N] "
    )?;
    stderr.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;

    let trimmed = line.trim().to_ascii_lowercase();
    if trimmed != "y" && trimmed != "yes" {
        anyhow::bail!("bootstrap platform key reset cancelled");
    }

    Ok(())
}

fn print_output(format: &OutputFormat, raw_key: &str) -> anyhow::Result<()> {
    match format {
        OutputFormat::Human => {
            println!(
                "New bootstrap platform API key (save this now, it will not be shown again):\n  {}",
                raw_key
            );
        }
        OutputFormat::Json => {
            let output = serde_json::json!({ "platform_key": raw_key });
            println!("{}", serde_json::to_string(&output)?);
        }
        OutputFormat::Quiet => {
            println!("{}", raw_key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults() {
        let opts = parse_args(&[]).unwrap();
        assert!(!opts.yes);
        assert_eq!(opts.format, OutputFormat::Human);
    }

    #[test]
    fn parse_args_yes() {
        let opts = parse_args(&["--yes".to_string()]).unwrap();
        assert!(opts.yes);
    }

    #[test]
    fn parse_args_json() {
        let opts = parse_args(&["--json".to_string()]).unwrap();
        assert_eq!(opts.format, OutputFormat::Json);
    }

    #[test]
    fn parse_args_quiet() {
        let opts = parse_args(&["--quiet".to_string()]).unwrap();
        assert_eq!(opts.format, OutputFormat::Quiet);
    }

    #[test]
    fn parse_args_rejects_json_and_quiet() {
        let err = parse_args(&["--json".to_string(), "--quiet".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn parse_args_rejects_unknown() {
        let err = parse_args(&["--foo".to_string()]).unwrap_err().to_string();
        assert!(err.contains("unknown argument"), "{err}");
    }

    #[test]
    fn confirm_accepts_yes() {
        let input = b"yes\n";
        let mut stderr = Vec::new();
        confirm_interactive(&mut &input[..], &mut stderr).unwrap();
    }

    #[test]
    fn confirm_rejects_no() {
        let input = b"no\n";
        let mut stderr = Vec::new();
        assert!(confirm_interactive(&mut &input[..], &mut stderr).is_err());
    }

    #[test]
    fn print_human_includes_key() {
        let mut out = Vec::new();
        print_output_to(&OutputFormat::Human, "rsk_test", &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("rsk_test"));
        assert!(s.contains("New bootstrap platform API key"));
    }

    #[test]
    fn print_json_is_structured() {
        let mut out = Vec::new();
        print_output_to(&OutputFormat::Json, "rsk_test", &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s.trim(), r#"{"platform_key":"rsk_test"}"#);
    }

    #[test]
    fn print_quiet_is_raw() {
        let mut out = Vec::new();
        print_output_to(&OutputFormat::Quiet, "rsk_test", &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s.trim(), "rsk_test");
    }

    fn print_output_to(
        format: &OutputFormat,
        raw_key: &str,
        writer: &mut dyn Write,
    ) -> anyhow::Result<()> {
        match format {
            OutputFormat::Human => {
                writeln!(
                    writer,
                    "New bootstrap platform API key (save this now, it will not be shown again):\n  {}",
                    raw_key
                )?;
            }
            OutputFormat::Json => {
                let output = serde_json::json!({ "platform_key": raw_key });
                writeln!(writer, "{}", serde_json::to_string(&output)?)?;
            }
            OutputFormat::Quiet => {
                writeln!(writer, "{}", raw_key)?;
            }
        }
        Ok(())
    }
}

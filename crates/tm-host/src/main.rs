//! `tm-host`: the standalone host binary (no GUI dependencies). The GUI can also act as a
//! per-user host itself with `terminal-manager --host`.

use tm_core::paths::HostMode;

const USAGE: &str = "\
Usage:
  tm-host [run]                 Run a host for the current user in the foreground
  tm-host run --service-paths   Run in the foreground with the service's pipe and data folder
                                (for checking a service setup; stop the service first)
  tm-host install --user ACCOUNT [--allow ACCOUNT_OR_GROUP]... [--password-stdin] [--no-start]
                                Install the Windows service (as administrator). Terminals run
                                as ACCOUNT. Administrators can always connect; --allow adds
                                more accounts or groups.
  tm-host uninstall             Stop and remove the Windows service (projects and logs are kept)
  tm-host service               Entry point used by the Service Control Manager";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => tm_host::run(HostMode::User, true).map_err(|e| e.to_string()),
        Some("run") => {
            let mode = if args.iter().any(|a| a == "--service-paths") { HostMode::Service } else { HostMode::User };
            tm_host::run(mode, true).map_err(|e| e.to_string())
        }
        Some("install") => install(&args[1..]),
        Some("uninstall") => uninstall(),
        Some("service") => service(),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return;
        }
        Some(other) => Err(format!("Unknown command `{other}`.\n\n{USAGE}")),
    };
    if let Err(e) = result {
        eprintln!("tm-host: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn install(args: &[String]) -> Result<(), String> {
    let mut account = None;
    let mut allow = Vec::new();
    let mut password_stdin = false;
    let mut start = true;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--user" => account = it.next().cloned(),
            "--allow" => allow.extend(it.next().cloned()),
            "--password-stdin" => password_stdin = true,
            "--no-start" => start = false,
            other => return Err(format!("Unknown option `{other}`.\n\n{USAGE}")),
        }
    }
    let account = account.ok_or_else(|| format!("--user ACCOUNT is required.\n\n{USAGE}"))?;
    let password = if password_stdin {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        line.trim_end_matches(['\r', '\n']).to_string()
    } else {
        let shown = tm_host::service::qualify_account(&account);
        rpassword::prompt_password(format!("Password for {shown}: ")).map_err(|e| e.to_string())?
    };
    tm_host::service::install(tm_host::service::InstallOptions { account, password, allow, start })
}

#[cfg(windows)]
fn uninstall() -> Result<(), String> {
    tm_host::service::uninstall()
}

#[cfg(windows)]
fn service() -> Result<(), String> {
    tm_host::service::dispatch().map_err(|e| {
        format!("{e}. `tm-host service` is started by Windows; use `tm-host run` to run in a console.")
    })
}

#[cfg(not(windows))]
fn install(_: &[String]) -> Result<(), String> {
    Err("Service install is Windows-only. On Linux, run `tm-host` under systemd (see README).".into())
}

#[cfg(not(windows))]
fn uninstall() -> Result<(), String> {
    install(&[])
}

#[cfg(not(windows))]
fn service() -> Result<(), String> {
    install(&[])
}

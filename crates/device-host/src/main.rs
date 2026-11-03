//! Console host for native macOS/Linux input and device-independent dry runs.
use device_host::{console::ConsoleApproval, inject, serve};
use protocol::{
    access::AccessStore,
    config::{Config, Role},
};
use std::{
    io::{self, BufRead},
    sync::Arc,
};

fn main() -> io::Result<()> {
    let mut dry_run = false;
    let mut config_path = None;
    let mut action = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("keyboard-it-host [--config PATH] [--dry-run] [--list-controllers | --revoke ID | --revoke-all]\n\nNative keyboard/mouse host: macOS (Accessibility), Linux X11 (XTEST), Windows.\nConsole commands: allow CODE, deny, list, revoke ID, revoke-all, quit.\nEvery pairing requires local code approval, including --dry-run.\nDefault config: host.toml next to the app config; KEYBOARD_IT_CONFIG overrides it.");
                return Ok(());
            }
            "--config" => {
                config_path = Some(
                    args.next()
                        .ok_or_else(|| io::Error::other("--config needs a path"))?,
                )
            }
            "--dry-run" => dry_run = true,
            "--list-controllers" | "--revoke-all" => {
                if action.is_some() {
                    return Err(io::Error::other("Choose one access-management action"));
                }
                action = Some((arg, None));
            }
            "--revoke" => {
                if action.is_some() {
                    return Err(io::Error::other("Choose one access-management action"));
                }
                let id = args
                    .next()
                    .ok_or_else(|| io::Error::other("--revoke needs a controller ID"))?;
                action = Some((arg, Some(id)));
            }
            _ => {
                return Err(io::Error::other(format!(
                    "Unknown argument {arg:?}; use --help"
                )))
            }
        }
    }
    // Controller and host can run on the same Mac without sharing identities,
    // selected targets or receiver-side access policy.
    if let Some(path) = config_path {
        std::env::set_var("KEYBOARD_IT_CONFIG", path);
    } else if std::env::var_os("KEYBOARD_IT_CONFIG").is_none() {
        std::env::set_var(
            "KEYBOARD_IT_CONFIG",
            Config::path()?.with_file_name("host.toml"),
        );
    }
    let path = Config::path()?;
    if path.is_relative() {
        std::env::set_var("KEYBOARD_IT_CONFIG", std::env::current_dir()?.join(path));
    }
    let mut cfg = Config::load()?.unwrap_or_default();
    cfg.role = Role::Receiver;
    let access = AccessStore::open(&cfg)?;
    if let Some((action, id)) = action {
        match action.as_str() {
            "--revoke" => access.revoke(id.as_deref().unwrap())?,
            "--revoke-all" => access.revoke_all()?,
            _ => list(&access),
        }
        return Ok(());
    }
    cfg.ensure_identity();
    cfg.save()?;
    if dry_run {
        inject::set_backend(Box::new(device_host::input::DryRun))?;
    } else {
        inject::enable_native()?;
    }
    println!(
        "{} — config: {}",
        if dry_run {
            "Dry run: no OS input"
        } else {
            "Native keyboard/mouse host"
        },
        Config::path()?.display()
    );
    let approval = Arc::new(ConsoleApproval::default());
    let decisions = approval.clone();
    let mut host = serve::start_with_access(
        &cfg,
        access.clone(),
        |state| println!("Session: {state:?}"),
        Arc::new(move |request| decisions.decide(request)),
    )?;
    println!("Commands: allow CODE, deny, list, revoke ID, revoke-all, quit");
    let result = (|| -> io::Result<()> {
        for line in io::stdin().lock().lines() {
            let line = line?;
            let parts: Vec<_> = line.split_whitespace().collect();
            match parts.as_slice() {
                ["allow", code] => println!(
                    "{}",
                    if approval.answer(Some(code)) {
                        "Approval delivered; waiting for pairing to finish"
                    } else {
                        "No matching pending request (wrong code or expired)"
                    }
                ),
                ["deny"] => {
                    approval.answer(None);
                }
                ["list"] => list(&access),
                ["revoke", id] => {
                    if let Err(e) = access.revoke(id) {
                        eprintln!("Access unchanged: {e}");
                    }
                }
                ["revoke-all"] => {
                    if let Err(e) = access.revoke_all() {
                        eprintln!("Access unchanged: {e}");
                    }
                }
                ["quit"] => break,
                [] => {}
                _ => eprintln!("Commands: allow CODE, deny, list, revoke ID, revoke-all, quit"),
            }
        }
        Ok(())
    })();
    approval.answer(None);
    host.stop();
    inject::disconnect();
    result
}
fn list(access: &AccessStore) {
    let controllers = access.controllers();
    if controllers.is_empty() {
        println!("No authorized controllers");
    }
    for c in controllers {
        println!(
            "{}  {:?}{}",
            c.id,
            c.name,
            if c.legacy {
                " (legacy shared-key group)"
            } else {
                ""
            }
        );
    }
}

const TEAM_URL: &str = "https://team-api.infra.rust-lang.org/v1/permissions/dev_desktop.json";

use clap::Parser;
use miniserde::Deserialize;
use std::collections::HashSet;
use std::process::Command;
use std::process::Output;

#[derive(Deserialize)]
struct All {
    github_users: Vec<String>,
}

fn cmd(cmd: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new(cmd).args(args).output()
}

const KEY_DIR: &str = "/etc/ssh/authorized_keys/";

#[derive(Parser)]
struct Args {
    #[clap(long, value_parser)]
    user_quota_gb: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut handle = curl::easy::Easy::new();
    handle
        .useragent("rust-lang/simpleinfra (infra@rust-lang.org)")
        .unwrap();
    handle.url(TEAM_URL).unwrap();
    let mut data = Vec::new();
    {
        let mut transfer = handle.transfer();
        transfer
            .write_function(|new_data| {
                data.extend_from_slice(new_data);
                Ok(new_data.len())
            })
            .unwrap();
        transfer.perform().unwrap();
    }

    let data = String::from_utf8(data).unwrap();
    let all: All = miniserde::json::from_str(&data).unwrap();
    let mut users = HashSet::new();
    for username in all.github_users {
        let url = format!("https://github.com/{}.keys", username);
        // Pick a user name that won't conflict with system users
        let username = format!("gh-{}", username);

        users.insert(username.clone());
        // Get the keys the user added to their github account.
        // Do this every time the cronjob runs so that they get their new keys if necessary.
        let mut keys = Vec::new();
        handle.url(&url).unwrap();
        {
            let mut transfer = handle.transfer();
            transfer
                .write_function(|new_data| {
                    keys.extend_from_slice(new_data);
                    Ok(new_data.len())
                })
                .unwrap();
            transfer.perform().expect("failed to download user's keys");
        }
        let keys = String::from_utf8(keys).unwrap();
        std::fs::write(format!("{}{}", KEY_DIR, username), keys)
            .expect("Failed to create key file");

        // Check if user exists
        let id = cmd("id", &[&username]).expect("failed to run `id` command");
        if id.status.success() {
            continue;
        }

        // If user does not exist, create it
        assert!(
            cmd("useradd", &["--create-home", &username])?
                .status
                .success(),
            "failed to create user"
        );

        // Get them ssh access
        assert!(
            cmd("usermod", &["-a", "-G", "dev-desktop-allow-ssh", &username])?
                .status
                .success(),
            "failed to give user ssh access"
        );

        // Set their default shell
        assert!(
            cmd("usermod", &["--shell", "/usr/bin/bash", &username])?
                .status
                .success(),
            "failed to set the default shell"
        );

        // Set a user quota
        let hard_limit = format!("{}G", args.user_quota_gb + 1);
        let soft_limit = format!("{}G", args.user_quota_gb);

        assert!(
            cmd(
                "setquota",
                &["-u", &username, &soft_limit, &hard_limit, "0", "0", "/"]
            )?
            .status
            .success(),
            "failed to set a user quota"
        );
    }

    // Delete all keys for users that weren't on the list
    for entry in std::fs::read_dir(KEY_DIR)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            if let Some(extension) = path.extension() {
                if extension == "keys" {
                    if let Some(stem) = path.file_stem() {
                        if let Some(stem) = stem.to_str() {
                            if stem.starts_with("gh-") && !users.contains(stem) {
                                std::fs::remove_file(path)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

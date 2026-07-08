use aws_config::BehaviorVersion;
use aws_sdk_ec2::config::ProvideCredentials;
use aws_sdk_ec2::error::ProvideErrorMetadata;
use aws_sdk_ec2::types::{Filter, Instance};
use aws_sdk_ec2::Client;
use std::process::Command;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let program = args.first().cloned().unwrap_or_else(|| "avm".to_string());

    if args.len() < 2 {
        eprintln!("Usage: {program} <start|status|version|ssh|tmux> [args...]");
        return Err("missing subcommand".into());
    }

    let subcommand = args[1].clone();
    let rest: Vec<String> = args[2..].to_vec();

    // At least one of AVM_INSTANCE (Name tag) or AVM_INSTANCE_ID (instance
    // ID) is required for every subcommand.
    let instance_name = std::env::var("AVM_INSTANCE").ok();
    let instance_id_override = std::env::var("AVM_INSTANCE_ID").ok();

    if instance_name.is_none() && instance_id_override.is_none() {
        eprintln!(
            "\x1b[31mError:\x1b[0m Set AVM_INSTANCE (the EC2 instance's Name tag) or \
             AVM_INSTANCE_ID (its instance ID) so {program} knows which instance to manage."
        );
        return Err("missing AVM_INSTANCE / AVM_INSTANCE_ID".into());
    }

    let sdk_config = aws_config::load_defaults(BehaviorVersion::latest()).await;

    if sdk_config.region().is_none() {
        eprintln!(
            "\x1b[31mError:\x1b[0m No AWS region configured.\n\
             Set the AWS_REGION environment variable, or configure a default region with \
             `aws configure`."
        );
        return Err("missing AWS region".into());
    }

    match sdk_config.credentials_provider() {
        Some(provider) if provider.provide_credentials().await.is_ok() => {}
        _ => {
            eprintln!(
                "\x1b[31mError:\x1b[0m No AWS credentials found.\n\
                 Set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY (and AWS_SESSION_TOKEN if \
                 applicable), run `aws configure` to create a credentials file, or run \
                 {program} from an environment with an IAM role attached."
            );
            return Err("missing AWS credentials".into());
        }
    }

    let client = Client::new(&sdk_config);

    let instance = find_instance(
        &client,
        instance_id_override.as_deref(),
        instance_name.as_deref(),
    )
    .await?
    .ok_or_else(|| {
        let target = instance_id_override
            .as_deref()
            .or(instance_name.as_deref())
            .unwrap_or_default();
        format!("instance `{target}` not found")
    })?;

    let instance_id = instance
        .instance_id
        .clone()
        .ok_or("instance has no instance ID")?;
    let display_name = display_name(&instance).unwrap_or_else(|| instance_id.clone());
    let zone = availability_zone(&instance);
    let status = status_of(&instance);
    let ip = ip_of(&instance);

    match subcommand.as_str() {
        "status" => {
            let simple = if status == "RUNNING" {
                "RUNNING"
            } else {
                "STOPPED"
            };
            println!("{simple}");
            Ok(())
        }

        "version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }

        "ssh" => {
            // AVM_USER is required for ssh.
            let user = require_avm_user()?;

            let ip =
                ip.ok_or_else(|| format!("instance `{display_name}` has no reachable IP address"))?;

            eprintln!(
                "\x1b[32mConnecting\x1b[0m to {user}@{ip} \
                 (instance `{display_name}` in {zone})"
            );

            let status = Command::new("ssh")
                .arg("-o")
                .arg("StrictHostKeyChecking=no")
                .arg("-o")
                .arg("UserKnownHostsFile=/dev/null")
                .arg(format!("{user}@{ip}"))
                .args(&rest)
                .status()?;

            std::process::exit(status.code().unwrap_or(1));
        }

        "tmux" => {
            let user = require_avm_user()?;

            // The first positional argument after "tmux" is the session name.
            let session_name = rest.first().ok_or_else(|| {
                eprintln!("\x1b[31mError:\x1b[0m Usage: {program} tmux <session-name>");
                "missing session-name"
            })?;

            if status != "RUNNING" {
                return Err(format!(
                    "instance `{display_name}` is not running (status: {status}). \
                     Use `{program} start` to start it first."
                )
                .into());
            }

            let ip =
                ip.ok_or_else(|| format!("instance `{display_name}` has no reachable IP address"))?;

            eprintln!(
                "\x1b[32mConnecting\x1b[0m to {user}@{ip} \
                 (instance `{display_name}` in {zone}, tmux session `{session_name}`)"
            );

            // -t allocates a pseudo-TTY, which tmux requires.
            let exit_status = Command::new("ssh")
                .arg("-t")
                .arg(format!("{user}@{ip}"))
                .arg(format!("tmux new -As {session_name}"))
                .status()?;

            std::process::exit(exit_status.code().unwrap_or(1));
        }

        "start" => {
            if status == "RUNNING" {
                eprintln!("Instance `{display_name}` is already RUNNING.");
            } else {
                eprintln!(
                    "\x1b[32mStarting\x1b[0m instance `{display_name}` in {zone} \
                     (current status: {status})..."
                );
                client
                    .start_instances()
                    .instance_ids(instance_id.clone())
                    .send()
                    .await?;
            }

            let timeout_secs: u64 = std::env::var("AVM_START_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(180);

            wait_for_ssh(&client, &instance_id, Duration::from_secs(timeout_secs)).await?;

            std::process::exit(0);
        }

        other => {
            eprintln!(
                "Unknown subcommand `{other}`. \
                 Usage: {program} <start|status|version|ssh|tmux> [args...]"
            );
            Err(format!("unknown subcommand: {other}").into())
        }
    }
}

fn require_avm_user() -> Result<String, Box<dyn std::error::Error>> {
    std::env::var("AVM_USER").map_err(|_| {
        eprintln!("\x1b[31mError:\x1b[0m AVM_USER environment variable is not set.");
        "missing AVM_USER".into()
    })
}

/// Look up an instance either directly by ID (`instance_id`, from
/// `AVM_INSTANCE_ID`) or by its `Name` tag (`instance_name`, from
/// `AVM_INSTANCE`). The ID takes precedence when both are set.
///
/// A Name-tag lookup that matches more than one non-terminated instance is
/// treated as an error: it prints every match and tells the user to set
/// `AVM_INSTANCE_ID` to disambiguate, rather than guessing.
async fn find_instance(
    client: &Client,
    instance_id: Option<&str>,
    instance_name: Option<&str>,
) -> Result<Option<Instance>, Box<dyn std::error::Error>> {
    if let Some(id) = instance_id {
        let result = client.describe_instances().instance_ids(id).send().await;

        let output = match result {
            Ok(output) => output,
            Err(err) => {
                if err.code() == Some("InvalidInstanceID.NotFound") {
                    return Ok(None);
                }
                return Err(Box::new(err));
            }
        };

        let instance = output
            .reservations()
            .iter()
            .flat_map(|r| r.instances())
            .next()
            .cloned();
        return Ok(instance);
    }

    let name = instance_name.expect("caller guarantees at least one of id/name is set");

    let output = client
        .describe_instances()
        .filters(Filter::builder().name("tag:Name").values(name).build())
        .filters(
            Filter::builder()
                .name("instance-state-name")
                .values("pending")
                .values("running")
                .values("shutting-down")
                .values("stopping")
                .values("stopped")
                .build(),
        )
        .send()
        .await?;

    let mut matches: Vec<Instance> = output
        .reservations()
        .iter()
        .flat_map(|r| r.instances())
        .cloned()
        .collect();

    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.remove(0))),
        _ => {
            eprintln!(
                "\x1b[31mError:\x1b[0m {} instances have a Name tag of `{name}`:\n",
                matches.len()
            );
            for instance in &matches {
                let id = instance.instance_id().unwrap_or("<unknown>");
                let zone = availability_zone(instance);
                let status = status_of(instance);
                eprintln!("  - {id}  ({zone}, {status})");
            }
            eprintln!(
                "\nSet AVM_INSTANCE_ID to one of the instance IDs above to disambiguate, e.g.:\n\
                 \n  export AVM_INSTANCE_ID={}\n",
                matches[0].instance_id().unwrap_or("i-xxxxxxxxxxxxxxxxx")
            );
            Err(format!("ambiguous Name tag `{name}`").into())
        }
    }
}

/// Poll the instance until it is RUNNING and we can open a TCP connection to
/// its SSH port, or until `timeout` elapses.
async fn wait_for_ssh(
    client: &Client,
    instance_id: &str,
    timeout: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut last_status = String::new();
    let mut last_ip: Option<String> = None;

    loop {
        if start.elapsed() >= timeout {
            return Err(format!(
                "timed out after {}s waiting for `{instance_id}` to accept SSH",
                timeout.as_secs()
            )
            .into());
        }

        let output = client
            .describe_instances()
            .instance_ids(instance_id)
            .send()
            .await?;

        let instance = output
            .reservations()
            .iter()
            .flat_map(|r| r.instances())
            .next()
            .ok_or_else(|| format!("instance `{instance_id}` disappeared while waiting"))?;

        let status = status_of(instance);
        if status != last_status {
            eprintln!("  instance status: {status}");
            last_status = status.clone();
        }

        let ip = ip_of(instance);

        if status == "RUNNING" {
            if let Some(ip) = ip.clone() {
                if last_ip.as_deref() != Some(ip.as_str()) {
                    eprintln!("  probing SSH on {ip}:22 ...");
                    last_ip = Some(ip.clone());
                }

                let probe = tokio::time::timeout(
                    Duration::from_secs(3),
                    tokio::net::TcpStream::connect(format!("{ip}:22")),
                )
                .await;

                if let Ok(Ok(_)) = probe {
                    eprintln!("\x1b[32mSSH is ready\x1b[0m on {ip}");
                    return Ok(ip);
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Uppercased EC2 instance state name, e.g. `RUNNING`, `STOPPED`, `PENDING`.
fn status_of(instance: &Instance) -> String {
    instance
        .state()
        .and_then(|s| s.name())
        .map(|n| n.as_str().to_uppercase())
        .unwrap_or_default()
}

/// Prefers the public IP address, falling back to the private IP.
fn ip_of(instance: &Instance) -> Option<String> {
    instance
        .public_ip_address()
        .or_else(|| instance.private_ip_address())
        .map(str::to_string)
}

fn availability_zone(instance: &Instance) -> String {
    instance
        .placement()
        .and_then(|p| p.availability_zone())
        .unwrap_or("unknown zone")
        .to_string()
}

fn display_name(instance: &Instance) -> Option<String> {
    instance
        .tags()
        .iter()
        .find(|t| t.key() == Some("Name"))
        .and_then(|t| t.value())
        .map(str::to_string)
}

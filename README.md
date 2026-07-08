# avm

A small command-line tool for managing a single AWS EC2 instance you use as a development or work machine. It can check the instance's status, start it on demand, wait for SSH to become available, and then drop you into an SSH session.

`avm` is intentionally minimal: it targets exactly one instance in one AWS region, configured entirely through environment variables.

## Features

- **`avm status`** — print whether the instance is `RUNNING` or `STOPPED`.
- **`avm start`** — start the instance (if it isn't already running) and wait for it to accept SSH connections.
- **`avm ssh`** — SSH directly to the instance's current IP without touching its power state.

Any extra arguments after the subcommand are passed through to `ssh`, so you can do things like `avm ssh -- ls ~` or `avm start -- tmux attach`.

## Installation

You need a recent Rust toolchain (stable) and Cargo.

### From crates.io (recommended)

Install the latest published release directly with Cargo:

```sh
cargo install ec2-vm
```

This will download, build, and place the `avm` binary in Cargo's bin directory (usually `~/.cargo/bin`). Make sure that directory is on your `PATH`.

To upgrade to a newer version later:

```sh
cargo install ec2-vm --force
```

### From source

Clone the repository and build it yourself:

```sh
git clone <this-repo>
cd avm
cargo build --release
```

The resulting binary will be at `target/release/avm`. Copy or symlink it onto your `PATH`, e.g.:

```sh
install -m 0755 target/release/avm ~/.local/bin/avm
```

Or install straight from the source directory:

```sh
cargo install --path .
```

## Authentication

`avm` uses the standard AWS SDK credential and region resolution chain — the same one the AWS CLI uses. Before running it, make sure one of the following is true:

- `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (and `AWS_SESSION_TOKEN` if applicable) are set, **or**
- You've run `aws configure` to create credentials at `~/.aws/credentials` and a default region at `~/.aws/config`, **or**
- You're running `avm` somewhere with an IAM role attached (an EC2 instance profile, ECS task role, etc.).

A region must also be resolvable — either via `AWS_REGION`/`AWS_DEFAULT_REGION` or a default region in `~/.aws/config`. If neither credentials nor a region can be resolved, `avm` refuses to run and tells you how to fix it.

The credentials you use must have sufficient permissions to describe and start the target instance — for example the `AmazonEC2FullAccess` managed policy, or a tighter custom policy with `ec2:DescribeInstances` and `ec2:StartInstances`.

## Configuration

All configuration is read from environment variables.

| Variable | Required for | Description |
| --- | --- | --- |
| `AVM_INSTANCE` | all commands* | The EC2 instance's `Name` tag. |
| `AVM_INSTANCE_ID` | all commands* | The EC2 instance ID (e.g. `i-0123456789abcdef0`). Takes precedence over `AVM_INSTANCE` and skips the Name-tag lookup entirely. |
| `AVM_USER` | `ssh`, `start`, `tmux` | Local username on the instance to SSH as. |
| `AVM_START_TIMEOUT` | `start` (optional) | Seconds to wait for SSH to come up. Default: `180`. |
| `AWS_REGION` / `AWS_DEFAULT_REGION` | all commands (if not set via `~/.aws/config`) | AWS region the instance lives in. |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | optional | Explicit AWS credentials, if not using `~/.aws/credentials` or an IAM role. |

\* At least one of `AVM_INSTANCE` or `AVM_INSTANCE_ID` must be set.

### Disambiguating instances with the same Name tag

`avm` looks the instance up by its `Name` tag by default, which means it doesn't need to know the instance ID up front. If more than one non-terminated instance shares that Name tag, `avm` refuses to guess: it prints every matching instance ID (with its availability zone and status) and exits with an error telling you to set `AVM_INSTANCE_ID` to the one you meant, e.g.:

```sh
export AVM_INSTANCE_ID=i-0123456789abcdef0
```

### Example

```sh
export AWS_REGION=us-east-1
export AVM_INSTANCE=dev-box
export AVM_USER=alice
```

## Usage

```sh
avm <start|status|ssh|tmux> [args...]
```

### `avm status`

Prints `RUNNING` or `STOPPED` (anything other than RUNNING is reported as STOPPED).

```sh
$ avm status
RUNNING
```

### `avm start`

Starts the instance if necessary, and polls until it's `RUNNING` and TCP port 22 is reachable. If the instance is already running it skips straight to the SSH probe.

```sh
$ avm start
Starting instance `dev-box` in us-east-1a (current status: STOPPED)...
  instance status: PENDING
  instance status: RUNNING
  probing SSH on 34.123.45.67:22 ...
SSH is ready on 34.123.45.67
```

You can pass extra arguments through to `ssh` when using `avm ssh` or `avm tmux`:

```sh
avm ssh -- -A       # forward your SSH agent
avm ssh -- uptime   # run a command instead of an interactive shell
```

If the instance doesn't become reachable within `AVM_START_TIMEOUT` seconds, `avm` exits with an error.

### `avm ssh`

SSH to the instance at its current IP without trying to start it. Useful when you know it's already running and don't want the extra status polling.

```sh
avm ssh
avm ssh -- uptime
```

### `avm tmux <session-name>`

SSH to the instance and attach to (or create) a named tmux session. Requires the instance to already be `RUNNING`.

```sh
avm tmux work
```

## How it picks an IP

For `ssh`, `start`, and `tmux`, `avm`:

1. Prefers the instance's public IP address.
2. Falls back to its private IP address if no public IP is assigned.

If neither is available, the command fails with an error.

## Exit codes

- `avm status` exits `0` on success.
- `avm ssh` and `avm tmux` propagate `ssh`'s exit code.
- Any configuration or API error causes a non-zero exit with a message on stderr.

## Troubleshooting

- **`Set AVM_INSTANCE ... or AVM_INSTANCE_ID ...`** — export at least one of the variables listed under [Configuration](#configuration).
- **`No AWS region configured.`** — set `AWS_REGION` or configure a default region with `aws configure`.
- **`No AWS credentials found.`** — set `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, run `aws configure`, or run `avm` from an environment with an IAM role attached.
- **`N instances have a Name tag of ...`** — multiple instances share that Name tag; set `AVM_INSTANCE_ID` to the specific instance ID printed in the error.
- **`instance \`X\` not found`** — double-check `AVM_INSTANCE`/`AVM_INSTANCE_ID` and `AWS_REGION`; the instance must exist in that region.
- **`timed out after Ns waiting for ... to accept SSH`** — the instance started but port 22 didn't open in time. Increase `AVM_START_TIMEOUT`, or check security groups / sshd on the instance.

## License

MIT

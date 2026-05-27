use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt as _, BufReader};

use awesome_axum_responses::*;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::info;

pub fn deploy_router(
    repo: impl Into<String>,
    service: impl Into<String>,
    dir: impl Into<String>,
) -> Router {
    Router::new()
        .route("/deploy", post(deploy_post))
        .layer(Extension(DeployConfig {
            repo: repo.into(),
            service: service.into(),
            dir: dir.into(),
        }))
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct DeployConfig {
    repo: String,
    service: String,
    dir: String,
}

#[derive(Serialize, Deserialize)]
struct Deploy {
    repository: DeployRepo,
}

#[derive(Serialize, Deserialize)]
struct DeployRepo {
    name: String,
}

async fn deploy_post(
    Extension(config): Extension<DeployConfig>,
    Json(deploy): Json<Deploy>,
) -> Result<impl IntoResponse> {
    info!("Deploying '{}' in '{}'", deploy.repository.name, config.dir);

    if deploy.repository.name != config.repo {
        return Err(anyhow!(
            "tried to deploy the wrong repo '{}' on '{}'",
            deploy.repository.name,
            config.repo
        )
        .into());
    }
    if !Path::new(&config.dir).exists() {
        return Err(anyhow!("repository directory doesn't exist").into());
    }
    if !Path::new(&config.dir).is_dir() {
        return Err(anyhow!("repository directory isn't a directory").into());
    }

    let mut ssh_agent = Command::new("ssh-agent")
        .arg("-s")
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("err running ssh-agent: {e}"))?;
    if let Some(stdout) = ssh_agent.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            info!("SSH: {:?}", line);
        }
    }

    let mut pull_output = String::new();
    let mut pull_command = Command::new("git")
        .arg("pull")
        .current_dir(&config.dir)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("err running git pull: {e}"))?;
    if let Some(stdout) = pull_command.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            info!("PULL: {:?}", line);
            pull_output += &line;
        }
    }
    pull_command.wait().await?;

    if let Some(pid) = ssh_agent.id() {
        info!(
            "KILL: {:?}",
            Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .output()
                .await
                .map_err(|e| anyhow!("err running kill ssh-agent: {e}"))?
        );
    }

    if !cfg!(feature = "always-build") && is_sub(pull_output.as_ref(), b"Already up to date.") {
        return Ok("Already up to date");
    }

    let mut build_command = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(&config.dir)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("err running cargo build: {e}"))?;
    if let Some(stdout) = build_command.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            info!("BUILD: {:?}", line);
        }
    }
    build_command.wait().await?;

    info!(
        "RESTART: {:?}",
        Command::new("systemctl")
            .arg("restart")
            .arg(&config.service)
            .output()
            .await
            .map_err(|e| anyhow!("err running systemctl restart: {e}"))?
    );
    Ok("Deployed")
}

fn is_sub<T: PartialEq>(haystack: &[T], needle: &[T]) -> bool {
    haystack.windows(needle.len()).any(|c| c == needle)
}

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt as _, BufReader};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, info};

pub fn deploy_router(repo: &str, service: &str) -> Router {
    Router::new()
        .route("/deploy", post(deploy_post))
        .layer(Extension(DeployConfig {
            repo: repo.into(),
            service: service.into(),
        }))
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct DeployConfig {
    repo: String,
    service: String,
}

#[derive(Serialize, Deserialize)]
struct Deploy {
    repository: DeployRepo,
}

#[derive(Serialize, Deserialize)]
struct DeployRepo {
    name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("tried to deploy a different repo: {0}")]
    WrongRepo(String),
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        error!(err = ?self, "responding with error");
        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
    }
}

async fn deploy_post(
    Extension(config): Extension<DeployConfig>,
    Json(deploy): Json<Deploy>,
) -> Result<impl IntoResponse, Error> {
    let dir = format!("/var/www/{}", config.repo);
    info!("Deploying '{}' in '{}'", deploy.repository.name, dir);

    if deploy.repository.name != config.repo {
        return Err(Error::WrongRepo(deploy.repository.name));
    };

    let mut ssh_agent = Command::new("ssh-agent")
        .arg("-s")
        .stdout(Stdio::piped())
        .spawn()?;
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
        .current_dir(dir.clone())
        .stdout(Stdio::piped())
        .spawn()?;
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
                .await?
        );
    }

    if is_sub(pull_output.as_ref(), b"Already up to date.") {
        return Ok("Already up to date");
    }

    let mut build_command = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(dir)
        .stdout(Stdio::piped())
        .spawn()?;
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
            .arg(config.service.clone())
            .output()
            .await?
    );
    Ok("Deployed")
}

fn is_sub<T: PartialEq>(haystack: &[T], needle: &[T]) -> bool {
    haystack.windows(needle.len()).any(|c| c == needle)
}

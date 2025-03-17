use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, error, warn};

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
    warn!("Deploying {}", deploy.repository.name);
    let dir = if deploy.repository.name == config.repo {
        format!("/var/www/{}", config.repo)
    } else {
        return Err(Error::WrongRepo(deploy.repository.name));
    };

    debug!(
        "{:?}",
        Command::new("eval").arg("`ssh-agent`").output().await?
    );
    debug!("{:?}", Command::new("cd").arg(dir.clone()).output().await?);
    let pull_output = Command::new("git").arg("pull").output().await?;
    debug!("{:?}", pull_output);
    debug!(
        "{:?}",
        Command::new("kill").arg("$SSH_AGENT_PID").output().await?
    );

    if is_sub(pull_output.stdout.as_ref(), b"Already up to date.") {
        return Ok("Already up to date");
    }

    debug!(
        "{:?}",
        Command::new("cargo")
            .arg("build")
            .arg("--release")
            .current_dir(dir)
            .output()
            .await?
    );
    debug!(
        "{:?}",
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

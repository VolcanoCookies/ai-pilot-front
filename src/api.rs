use std::env;

use client::models::{AiPilot, MatchResult};
use lazy_static::lazy_static;
use regex::Regex;
use rocket::{Data, Route, State, data::ToByteUnit, http::Status, serde::json::Json};
use rocket_okapi::openapi;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AIP_CONFIG, SqliteClient, api_error::ApiErrors, cookie::ApiUser, model::UserToken};

#[openapi]
#[get("/healthz")]
fn api_health_check() -> &'static str {
    "OK"
}

lazy_static! {
    static ref NAME_REGEX: Regex =
        Regex::new(r"^\w{3,32}$").expect("Failed to compile regex for name validation");
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GetAiPilotResponse {
    pilots: Vec<AiPilot>,
}

#[openapi]
#[get("/aipilot?<name>")]
async fn api_get_ai_pilots(
    _user: ApiUser,
    name: Option<String>,
) -> Result<Json<GetAiPilotResponse>, ApiErrors> {
    let pilots = client::apis::default_api::get_ai_pilots(&AIP_CONFIG, name.as_deref(), None)
        .await
        .map_err(|e| {
            log::error!("Failed to get AIPilots: {}", e);
            ApiErrors::InternalError("Failed to get AIPilots".into())
        })?;

    let pilots = pilots.into_iter().collect::<Vec<_>>();

    Ok(Json(GetAiPilotResponse { pilots }))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct GetMatchResponse {
    matches: Vec<MatchResult>,
}

#[openapi]
#[get("/matches?<name>")]
async fn api_get_matches(
    _user: ApiUser,
    name: Option<String>,
) -> Result<Json<GetMatchResponse>, ApiErrors> {
    let matches =
        client::apis::default_api::get_match_results(&AIP_CONFIG, name.as_deref(), None, None)
            .await
            .map_err(|e| {
                log::error!("Failed to get match results: {}", e);
                ApiErrors::InternalError("Failed to get match results".into())
            })?;

    let matches = matches.into_iter().collect::<Vec<_>>();

    Ok(Json(GetMatchResponse { matches }))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PostAiPilotResponse {
    upload_id: Uuid,
    version: i32,
}

#[openapi]
#[post("/aipilot/upload?<name>", data = "<data>")]
async fn api_upload_ai_pilot(
    user: ApiUser,
    name: String,
    data: Data<'_>,
) -> Result<Json<PostAiPilotResponse>, ApiErrors> {
    if !NAME_REGEX.is_match(&name) {
        return Err(ApiErrors::BadRequest("Invalid name format".into()));
    }

    let data = data.open(25.mebibytes()).into_bytes().await.map_err(|e| {
        log::error!("Failed to read data: {}", e);
        ApiErrors::InternalError("Failed to read data".into())
    })?;

    let res = client::apis::default_api::upload_ai_pilot(
        &AIP_CONFIG,
        &name,
        data.value,
        Some(&user.discord_id),
    )
    .await
    .map_err(|e| {
        log::error!("Failed to upload AIPilot: {}", e);
        ApiErrors::InternalError("Failed to upload AIPilot".into())
    })?;

    Ok(Json(PostAiPilotResponse {
        upload_id: res.upload_id,
        version: res.version,
    }))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct CreateUserToken {
    name: String,
    expires_at: Option<i64>,
}

#[openapi]
#[post("/user_token", data = "<body>")]
async fn api_create_user_token(
    user: ApiUser,
    body: Json<CreateUserToken>,
    client: &State<SqliteClient>,
) -> Result<Json<UserToken>, ApiErrors> {
    let CreateUserToken { name, expires_at } = body.into_inner();

    let expires_at = expires_at
        .map(|ts| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0).ok_or_else(|| {
                log::error!("Invalid timestamp for expires_at: {}", ts);
                ApiErrors::BadRequest("Invalid timestamp for expires_at".into())
            })
        })
        .transpose()?;

    let token = UserToken::insert_user_token(name, user.id, expires_at, client)
        .await
        .map_err(|e| {
            log::error!("Failed to create user token: {}", e);
            ApiErrors::InternalError("Failed to create user token".into())
        })?;

    Ok(Json(token))
}

#[openapi]
#[delete("/user_token/<token_id>")]
async fn api_delete_user_token(
    user: ApiUser,
    token_id: i64,
    client: &State<SqliteClient>,
) -> Result<Status, ApiErrors> {
    UserToken::delete_by_id_and_user_id(token_id, user.id, client)
        .await
        .map_err(|e| {
            log::error!("Failed to delete user token: {}", e);
            ApiErrors::InternalError("Failed to delete user token".into())
        })?;

    Ok(Status::NoContent)
}

pub fn routes() -> Vec<Route> {
    openapi_get_routes![
        api_health_check,
        api_get_ai_pilots,
        api_get_matches,
        api_upload_ai_pilot,
        api_create_user_token,
        api_delete_user_token,
    ]
}

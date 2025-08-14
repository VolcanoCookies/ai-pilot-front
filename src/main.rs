pub mod api;
pub mod api_error;
pub mod cookie;
pub mod model;
pub mod util;

use std::{env, str::FromStr, time::Duration};

use client::{
    apis::configuration::{ApiKey, Configuration},
    models::match_result::Winner,
};
use lazy_static::lazy_static;
use moka::future::Cache;
use rocket::{
    fs::{relative, FileServer}, futures::future::join_all, http::{Cookie, CookieJar, Status}, response::Redirect, State
};
use rocket_dyn_templates::{Template, context};
use rocket_okapi::{
    rapidoc::{GeneralConfig, HideShowConfig, RapiDocConfig, make_rapidoc},
    settings::UrlObject,
};
use serde::{Deserialize, Serialize};

use crate::{
    api_error::ApiErrors,
    cookie::ApiUser,
    model::{User, UserToken},
    util::{discord_avatar_url, format_date_time},
};

#[macro_use]
extern crate rocket;
#[macro_use]
extern crate rocket_okapi;

type SqliteClient = sqlx::Pool<sqlx::Sqlite>;

lazy_static! {
    static ref AIP_CONFIG: Configuration = Configuration {
        base_path: env::var("AIP_API_BASE_URL").expect("AIP_API_BASE_URL must be set"),
        user_agent: Some("api-front/1.0".to_string()),
        api_key: Some(ApiKey {
            prefix: None,
            key: env::var("AIP_API_KEY").expect("AIP_API_KEY must be set")
        }),
        ..Default::default()
    };
    static ref PILOT_NAME_CACHE: Cache<uuid::Uuid, String> =
        Cache::builder().max_capacity(2048).build();
    static ref DISCORD_USER_CACHE: Cache<String, DiscordUserInfo> = Cache::builder()
        .max_capacity(2048)
        .time_to_live(Duration::from_secs(60 * 60 * 24))
        .build();
}

#[derive(Deserialize, Clone)]
struct DiscordUserInfo {
    id: String,
    username: String,
    avatar: String,
}

async fn sso_fetch_discord_username(id: &str) -> Option<DiscordUserInfo> {
    let client = reqwest::Client::default();
    client
        .get(format!("https://sso.isan.to/uinfo/{}", id))
        .send()
        .await
        .ok()?
        .json::<DiscordUserInfo>()
        .await
        .ok()
}

async fn get_discord_user(discord_id: String) -> Option<DiscordUserInfo> {
    DISCORD_USER_CACHE
        .optionally_get_with(discord_id.clone(), sso_fetch_discord_username(&discord_id))
        .await
}

#[get("/")]
async fn index_page(apiuser: ApiUser, client: &State<SqliteClient>) -> Template {
    let user = User::get_by_id(apiuser.id, client).await.ok();

    Template::render(
        "index",
        context! {
            user: user.map(|u| context!{ id: u.id, username: u.username, avatar_url: discord_avatar_url(&u.discord_id, &u.avatar_url) }),
        },
    )
}

// Partials: Home Pilots
#[get("/partials/home/pilots")]
async fn partial_home_pilots(user: ApiUser) -> Result<Template, ApiErrors> {
    // Fetch pilots owned by the user
    let mut pilots = client::apis::default_api::get_ai_pilots(&AIP_CONFIG, None, None)
        .await
        .map_err(|e| {
            log::error!("Failed to get AIPilots: {}", e);
            ApiErrors::InternalError("Failed to get AIPilots".into())
        })?;

    join_all(
        pilots
            .iter()
            .map(|p| PILOT_NAME_CACHE.insert(p.id, p.name.clone())),
    )
    .await;

    pilots.sort_by_key(|p| p.owner_id != user.discord_id.to_string());

    let pilots_ctx: Vec<_> = join_all(pilots.into_iter().map(async |p| {
        let is_own = p.owner_id == user.discord_id.to_string();
        let username = if is_own {
            "You".to_string()
        } else {
            get_discord_user(p.owner_id.clone())
                .await
                .map(|u| u.username)
                .unwrap_or_else(|| p.owner_id.clone())
        };

        context! {
            id: p.id.to_string(),
            name: p.name,
            current: context! { version: p.current.version },
            creator: username,
            is_own: is_own,
        }
    }))
    .await;

    Ok(Template::render(
        "partials/home_pilots",
        context! { pilots: pilots_ctx },
    ))
}

// Partials: Home Matches (recent)
#[get("/partials/home/matches")]
async fn partial_home_matches() -> Result<Template, ApiErrors> {
    let matches = client::apis::default_api::get_match_results(&AIP_CONFIG, None, None, None)
        .await
        .map_err(|e| {
            log::error!("Failed to get match results: {}", e);
            ApiErrors::InternalError("Failed to get match results".into())
        })?;

    let matches_ctx: Vec<_> = join_all(matches.into_iter().map(async |m| {
        let team_a_name = PILOT_NAME_CACHE
            .get(&m.team_a.aip_id)
            .await
            .unwrap_or(m.team_a.aip_id.to_string());
        let team_b_name = PILOT_NAME_CACHE
            .get(&m.team_b.aip_id)
            .await
            .unwrap_or(m.team_b.aip_id.to_string());

        let download_url = if let Some(reply_id) = m.reply_id {
            Some(format!("{}/replay?matchId={}", AIP_CONFIG.base_path, reply_id))
        } else {
            None
        };

        context! {
            id: m.id.to_string(),
            created_at: format_date_time(&chrono::DateTime::<chrono::Utc>::from_timestamp(m.created_at / 1_000, 0).unwrap_or_default()),
            is_manual: m.manual_run,
            team_a: context! { winner: m.winner == Winner::TeamA, aip_id: m.team_a.aip_id.to_string(), aip_name: team_a_name.clone(), version: m.team_a.version },
            team_b: context! { winner: m.winner == Winner::TeamB, aip_id: m.team_b.aip_id.to_string(), aip_name: team_b_name.clone(), version: m.team_b.version },
            winner: match m.winner {
                Winner::TeamA => team_a_name,
                Winner::TeamB => team_b_name,
                Winner::Unknown => "Unknown".to_string(),
            },
            download_url: download_url,
        }
    }))
    .await;

    Ok(Template::render(
        "partials/home_matches",
        context! { matches: matches_ctx },
    ))
}

#[get("/login")]
async fn login() -> Result<Redirect, ApiErrors> {
    let base_url =
        env::var("BASE_URL").map_err(|_| ApiErrors::InternalError("BASE_URL not set".into()))?;
    let callback_url = format!("{}/login_callback", base_url);
    Ok(Redirect::to(format!(
        "https://sso.isan.to/login?service={}",
        callback_url
    )))
}

#[derive(Debug, Serialize, Deserialize)]
struct DiscordServerRole {
    #[serde(rename = "discordId")]
    discord_id: String,
    roles: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetUserDataResponse {
    id: String,
    username: String,
    avatar: String,
    roles: Vec<DiscordServerRole>,
}

#[get("/login_callback?<code>")]
async fn login_callback(
    code: &str,
    cookies: &CookieJar<'_>,
    client: &State<SqliteClient>,
) -> Result<Redirect, ApiErrors> {
    let res = reqwest::Client::new()
        .get(format!("https://sso.isan.to/getuser/{}", code))
        .send()
        .await
        .map_err(|e| {
            log::error!("Failed to fetch login callback: {}", e);
            ApiErrors::InternalError("Failed to fetch login callback".into())
        })?;

    let data = res.json::<GetUserDataResponse>().await.map_err(|e| {
        log::error!("Failed to parse login callback response: {}", e);
        ApiErrors::InternalError("Failed to parse login callback response".into())
    })?;

    let user = User::upsert_by_discord_id(&data.id, &data.username, &data.avatar, &**client)
        .await
        .map_err(|e| {
            log::error!("Failed to upsert user: {}", e);
            ApiErrors::InternalError("Failed to upsert user".into())
        })?;

    let cookie_str = serde_json::to_string(&ApiUser {
        id: user.id,
        discord_id: user.discord_id,
    })
    .map_err(|e| {
        log::error!("Failed to serialize user data: {}", e);
        ApiErrors::InternalError("Failed to serialize user data".into())
    })?;

    cookies.add_private(Cookie::new("auth", cookie_str));

    // Needed since cookies are queued for redirects
    Ok(Redirect::found(uri!("/login_callback_redirect")))
}

#[get("/logout")]
async fn logout(cookies: &CookieJar<'_>) -> Redirect {
    cookies.remove_private("auth");
    Redirect::to(uri!("/login"))
}

#[get("/login_callback_redirect")]
async fn login_callback_redirect_page() -> Template {
    Template::render("login_callback", context! {})
}

#[get("/user_tokens")]
async fn user_tokens_page(
    user: ApiUser,
    client: &State<SqliteClient>,
) -> Result<Template, ApiErrors> {
    let tokens = UserToken::get_by_user_id(user.id, client)
        .await
        .map_err(|_| ApiErrors::InternalError("Failed to fetch user tokens".into()))?;

    let u = crate::model::User::get_by_id(user.id, client).await.ok();

    Ok(Template::render(
        "user_tokens",
        context! {
            tokens: tokens.iter().map(|t| context! {
                id: t.id,
                name: t.name.clone(),
                token: t.token.clone(),
                created_at: format_date_time(&t.created_at),
                expires_at: t.expires_at.map(|d| format_date_time(&d)),
            }).collect::<Vec<_>>(),
            user: u.as_ref().map(|u| context!{ id: u.id, username: u.username.clone(), avatar_url: discord_avatar_url(&u.discord_id, &u.avatar_url) }),
        },
    ))
}

#[get("/upload?<name>")]
async fn upload_page(
    user: ApiUser,
    client: &State<SqliteClient>,
    name: Option<String>,
) -> Result<Template, ApiErrors> {
    let u = User::get_by_id(user.id, client).await.ok();

    let pilots = client::apis::default_api::get_ai_pilots(&AIP_CONFIG, None, None)
        .await
        .map_err(|e| {
            log::error!("Failed to get AIPilots: {}", e);
            ApiErrors::InternalError("Failed to get AIPilots".into())
        })?;

    let mut my_names = Vec::new();
    let mut other_names = Vec::new();

    for p in pilots.into_iter() {
        if p.owner_id == user.discord_id.to_string() {
            my_names.push(p.name);
        } else {
            other_names.push(p.name);
        }
    }

    Ok(Template::render(
        "upload",
        context! {
            my_names: my_names,
            other_names: other_names,
            preset_name: name,
            user: u.map(|u| context!{ id: u.id, username: u.username, avatar_url: discord_avatar_url(&u.discord_id, &u.avatar_url) }),
        },
    ))
}

#[get("/pilot/<pilot_name>")]
async fn pilot_stats_page(
    user: ApiUser,
    pilot_name: &str,
    client: &State<SqliteClient>,
) -> Result<Template, ApiErrors> {
    let u = User::get_by_id(user.id, client).await.ok();

    // Get all pilots to find the specific one by name
    let pilots = client::apis::default_api::get_ai_pilots(&AIP_CONFIG, None, None)
        .await
        .map_err(|e| {
            log::error!("Failed to get AI Pilots: {}", e);
            ApiErrors::InternalError("Failed to get pilots".into())
        })?;

    let pilot = pilots.into_iter()
        .find(|p| p.name == pilot_name)
        .ok_or_else(|| ApiErrors::NotFound("Pilot not found".into()))?;

    // Get all matches (we'll filter client-side for now)
    let all_matches = client::apis::default_api::get_match_results(&AIP_CONFIG, None, None, None)
        .await
        .map_err(|e| {
            log::error!("Failed to get matches: {}", e);
            ApiErrors::InternalError("Failed to get matches".into())
        })?;

    // Filter matches that involve this pilot
    let matches: Vec<_> = all_matches.into_iter()
        .filter(|m| m.team_a.aip_id == pilot.id || m.team_b.aip_id == pilot.id)
        .collect();

    // Calculate overall stats
    let total_matches = matches.len();
    let wins = matches.iter().filter(|m| {
        (m.team_a.aip_id == pilot.id && m.winner == Winner::TeamA) ||
        (m.team_b.aip_id == pilot.id && m.winner == Winner::TeamB)
    }).count();
    let losses = total_matches - wins;
    let win_rate = if total_matches > 0 { wins as f32 / total_matches as f32 * 100.0 } else { 0.0 };

    // Group matches by opponent
    let mut opponent_stats = std::collections::HashMap::new();
    for m in &matches {
        let (opponent_id, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.winner == Winner::TeamB)
        };
        
        let opponent_name = PILOT_NAME_CACHE
            .get(&opponent_id)
            .await
            .unwrap_or(opponent_id.to_string());
        
        let stats = opponent_stats.entry(opponent_name).or_insert((0, 0, 0)); // (wins, losses, total)
        if won {
            stats.0 += 1;
        } else {
            stats.1 += 1;
        }
        stats.2 += 1;
    }

    // Convert to sorted vector
    let mut opponents: Vec<_> = opponent_stats.into_iter().map(|(name, (wins, losses, total))| {
        (name, wins, losses, total, if total > 0 { wins as f32 / total as f32 * 100.0 } else { 0.0 })
    }).collect();
    opponents.sort_by(|a, b| b.3.cmp(&a.3)); // Sort by total matches

    let opponents_ctx: Vec<_> = opponents.into_iter().map(|(name, wins, losses, total, win_rate)| {
        context! {
            name: name,
            wins: wins,
            losses: losses,
            total: total,
            win_rate: format!("{:.0}", win_rate),
        }
    }).collect();

    // Group matches by version
    let mut version_stats = std::collections::HashMap::new();
    for m in &matches {
        let (version, won) = if m.team_a.aip_id == pilot.id {
            (m.team_a.version, m.winner == Winner::TeamA)
        } else {
            (m.team_b.version, m.winner == Winner::TeamB)
        };
        
        let stats = version_stats.entry(version).or_insert((0, 0, 0)); // (wins, losses, total)
        if won {
            stats.0 += 1;
        } else {
            stats.1 += 1;
        }
        stats.2 += 1;
    }

    // Convert to sorted vector (by version desc)
    let mut versions: Vec<_> = version_stats.into_iter().map(|(version, (wins, losses, total))| {
        (version, wins, losses, total, if total > 0 { wins as f32 / total as f32 * 100.0 } else { 0.0 })
    }).collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0)); // Sort by version descending

    let versions_ctx: Vec<_> = versions.iter().enumerate().map(|(index, (version, wins, losses, total, win_rate))| {
        let trend = if index < versions.len() - 1 {
            let prev_win_rate = versions[index + 1].4;
            if *win_rate > prev_win_rate {
                "up"
            } else if *win_rate < prev_win_rate {
                "down"
            } else {
                "neutral"
            }
        } else {
            "neutral"
        };

        context! {
            version: version,
            wins: wins,
            losses: losses,
            total: total,
            win_rate: format!("{:.0}", win_rate),
            trend: trend,
        }
    }).collect();

    // Recent matches (last 10) - sort by created_at descending to get latest first
    let mut sorted_matches = matches.clone();
    sorted_matches.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let recent_matches = join_all(sorted_matches.iter().take(10).map(async |m| {
        let (opponent_id, opponent_version, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.team_b.version, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.team_a.version, m.winner == Winner::TeamB)
        };
        
        let opponent_name = PILOT_NAME_CACHE
            .get(&opponent_id)
            .await
            .unwrap_or(opponent_id.to_string());
        
        context! {
            opponent: opponent_name,
            opponent_version: opponent_version,
            won: won,
            created_at: format_date_time(&chrono::DateTime::<chrono::Utc>::from_timestamp(m.created_at / 1_000, 0).unwrap_or_default()),
            is_manual: m.manual_run,
        }
    })).await;

    let pilot_name = pilot.name.clone();
    let pilot_owner_id = pilot.owner_id.clone();
    let pilot_current_version = pilot.current.version;
    let is_own_pilot = pilot_owner_id == user.discord_id.to_string();

    // Get creator info from Discord cache
    let creator_info = get_discord_user(pilot_owner_id.clone()).await;
    let creator_name = creator_info
        .as_ref()
        .map(|info| info.username.clone())
        .unwrap_or_else(|| pilot_owner_id.clone());
    let creator_avatar = creator_info
        .as_ref()
        .map(|info| discord_avatar_url(&pilot_owner_id, &info.avatar));

    Ok(Template::render(
        "pilot_stats",
        context! {
            pilot: context! {
                name: pilot_name.clone(),
                creator: creator_name,
                creator_avatar: creator_avatar,
                owner_id: pilot_owner_id,
                current_version: pilot_current_version,
                is_own: is_own_pilot,
            },
            overall_stats: context! {
                total_matches: total_matches,
                wins: wins,
                losses: losses,
                win_rate: format!("{:.0}", win_rate),
            },
            opponents: opponents_ctx,
            versions: versions_ctx,
            recent_matches: recent_matches,
            // Pass raw matches data for JavaScript filtering
            all_matches_json: serde_json::to_string(&matches).unwrap_or_default(),
            pilot_id: pilot.id.to_string(),
            user: u.map(|u| context!{ id: u.id, username: u.username, avatar_url: discord_avatar_url(&u.discord_id, &u.avatar_url) }),
        },
    ))
}

// New endpoint for version-specific data
#[get("/pilot/<pilot_name>/version/<version>")]
async fn pilot_version_stats(
    _user: ApiUser,
    pilot_name: &str,
    version: i32,
) -> Result<Template, ApiErrors> {
    // Get all pilots to find the specific one by name
    let pilots = client::apis::default_api::get_ai_pilots(&AIP_CONFIG, None, None)
        .await
        .map_err(|e| {
            log::error!("Failed to get AI Pilots: {}", e);
            ApiErrors::InternalError("Failed to get pilots".into())
        })?;

    let pilot = pilots.into_iter()
        .find(|p| p.name == pilot_name)
        .ok_or_else(|| ApiErrors::NotFound("Pilot not found".into()))?;

    // Get all matches and filter for this pilot and version
    let all_matches = client::apis::default_api::get_match_results(&AIP_CONFIG, None, None, None)
        .await
        .map_err(|e| {
            log::error!("Failed to get matches: {}", e);
            ApiErrors::InternalError("Failed to get matches".into())
        })?;

    let version_matches: Vec<_> = all_matches.into_iter()
        .filter(|m| {
            (m.team_a.aip_id == pilot.id && m.team_a.version == version) ||
            (m.team_b.aip_id == pilot.id && m.team_b.version == version)
        })
        .collect();

    // Calculate version-specific opponent stats
    let mut opponent_stats = std::collections::HashMap::new();
    for m in &version_matches {
        let (opponent_id, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.winner == Winner::TeamB)
        };
        
        let opponent_name = PILOT_NAME_CACHE
            .get(&opponent_id)
            .await
            .unwrap_or(opponent_id.to_string());
        
        let stats = opponent_stats.entry(opponent_name).or_insert((0, 0, 0));
        if won {
            stats.0 += 1;
        } else {
            stats.1 += 1;
        }
        stats.2 += 1;
    }

    let mut opponents: Vec<_> = opponent_stats.into_iter().map(|(name, (wins, losses, total))| {
        (name, wins, losses, total, if total > 0 { wins as f32 / total as f32 * 100.0 } else { 0.0 })
    }).collect();
    opponents.sort_by(|a, b| b.3.cmp(&a.3));

    let opponents_ctx: Vec<_> = opponents.into_iter().map(|(name, wins, losses, total, win_rate)| {
        context! {
            name: name,
            wins: wins,
            losses: losses,
            total: total,
            win_rate: format!("{:.0}", win_rate),
        }
    }).collect();

    // Recent matches for this version
    let mut sorted_matches = version_matches.clone();
    sorted_matches.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let recent_matches = join_all(sorted_matches.iter().take(10).map(async |m| {
        let (opponent_id, opponent_version, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.team_b.version, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.team_a.version, m.winner == Winner::TeamB)
        };
        
        let opponent_name = PILOT_NAME_CACHE
            .get(&opponent_id)
            .await
            .unwrap_or(opponent_id.to_string());
        
        context! {
            opponent: opponent_name,
            opponent_version: opponent_version,
            won: won,
            created_at: format_date_time(&chrono::DateTime::<chrono::Utc>::from_timestamp(m.created_at / 1_000, 0).unwrap_or_default()),
            is_manual: m.manual_run,
        }
    })).await;

    // Calculate version-specific overall stats
    let total_matches = version_matches.len();
    let wins = version_matches.iter().filter(|m| {
        (m.team_a.aip_id == pilot.id && m.winner == Winner::TeamA) ||
        (m.team_b.aip_id == pilot.id && m.winner == Winner::TeamB)
    }).count();
    let losses = total_matches - wins;
    let win_rate = if total_matches > 0 { wins as f32 / total_matches as f32 * 100.0 } else { 0.0 };

    Ok(Template::render(
        "partials/version_stats",
        context! {
            overall_stats: context! {
                total_matches: total_matches,
                wins: wins,
                losses: losses,
                win_rate: format!("{:.0}", win_rate),
            },
            opponents: opponents_ctx,
            recent_matches: recent_matches,
        },
    ))
}

// Helper function to render error pages
fn render_error_page(code: u16, message: &str) -> Template {
    Template::render("error", context! {
        code: code.to_string(),
        message: message
    })
}

#[catch(401)]
fn unauthroized_catcher(_status: Status, _req: &rocket::Request<'_>) -> Redirect {
    Redirect::to("/login")
}

#[catch(404)]
fn not_found_catcher(_status: Status, _req: &rocket::Request<'_>) -> Template {
    render_error_page(404, "Not Found")
}

#[catch(500)]
fn internal_error_catcher(_status: Status, _req: &rocket::Request<'_>) -> Template {
    render_error_page(500, "Internal Server Error")
}

#[catch(default)]
fn default_catcher(status: Status, _req: &rocket::Request<'_>) -> Template {
    let message = match status.code {
        400 => "Bad Request",
        403 => "Forbidden",
        405 => "Method Not Allowed",
        422 => "Unprocessable Entity",
        _ => "Error"
    };
    
    render_error_page(status.code, message)
}

#[launch]
async fn rocket() -> _ {
    simple_logger::init_with_level(log::Level::Info).expect("Failed to initialize logger");

    let _ = dotenvy::dotenv();

    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let opts = sqlx::sqlite::SqliteConnectOptions::from_str(&url)
        .expect("Failed to parse DATABASE_URL")
        .create_if_missing(true);
    let client = sqlx::sqlite::SqlitePool::connect_with(opts)
        .await
        .expect("Failed to connect to database");
    sqlx::migrate!("./migrations")
        .run(&client)
        .await
        .expect("Failed to run migrations");

    rocket::build()
        .manage(client)
        .mount("/api", api::routes())
        .mount("/static", FileServer::from(relative!("public")))
        .mount(
            "/",
            routes![
                index_page,
                partial_home_pilots,
                partial_home_matches,
                user_tokens_page,
                upload_page,
                pilot_stats_page,
                pilot_version_stats,
                login_callback_redirect_page,
                login,
                login_callback,
                logout,
            ],
        )
        .mount(
            "/rapidoc",
            make_rapidoc(&RapiDocConfig {
                general: GeneralConfig {
                    spec_urls: vec![UrlObject::new("General", "../api/openapi.json")],
                    ..Default::default()
                },
                hide_show: HideShowConfig {
                    allow_spec_url_load: false,
                    allow_spec_file_load: false,
                    ..Default::default()
                },
                ..Default::default()
            }),
        )
        .register("/", catchers![unauthroized_catcher, not_found_catcher, internal_error_catcher, default_catcher])
        .attach(Template::fairing())
}

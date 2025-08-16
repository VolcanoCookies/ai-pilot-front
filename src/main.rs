pub mod api;
pub mod api_client;
pub mod api_error;
pub mod cookie;
pub mod model;
pub mod sso_client;
pub mod util;

use std::{env, str::FromStr};

use client::models::match_result::Winner;
use rocket::{
    State,
    fs::{FileServer, relative},
    futures::future::join_all,
    http::{Cookie, CookieJar, Status},
    response::Redirect,
    tokio::spawn,
};
use rocket_dyn_templates::{Template, context};
use rocket_okapi::{
    rapidoc::{GeneralConfig, HideShowConfig, RapiDocConfig, make_rapidoc},
    settings::UrlObject,
};

use crate::{
    api_client::ApiClient,
    api_error::ApiErrors,
    cookie::ApiUser,
    model::{User, UserToken},
    sso_client::SSOClient,
    util::{build_info_ctx, discord_avatar_url, format_date_time},
};

#[macro_use]
extern crate rocket;
#[macro_use]
extern crate rocket_okapi;

type SqliteClient = sqlx::Pool<sqlx::Sqlite>;

#[get("/")]
async fn index_page(user: Option<ApiUser>) -> Template {
    Template::render(
        "index",
        context! {
            user: user,
            build_info: build_info_ctx()
        },
    )
}

// Partials: Home Pilots
#[get("/partials/home/pilots")]
async fn partial_home_pilots(
    user: Option<ApiUser>,
    sso_client: &State<SSOClient>,
    api_client: &State<ApiClient>,
) -> Result<Template, ApiErrors> {
    // Fetch pilots owned by the user
    let mut pilots = api_client.get_pilots().await;

    if let Some(user) = &user {
        pilots.sort_by_key(|p| p.owner_id != user.discord_id);
    }

    let pilots_ctx: Vec<_> = join_all(pilots.into_iter().map(async |p| {
        let is_own = if let Some(user) = &user {
            p.owner_id == user.discord_id
        } else {
            false
        };
        let username = if is_own {
            "You".to_string()
        } else {
            sso_client
                .get_user(&p.owner_id)
                .await
                .map(|u| u.username)
                .unwrap_or_else(|| p.owner_id.clone())
        };

        let creator_user = sso_client.get_user(&p.owner_id).await;

        context! {
            id: p.id.to_string(),
            name: p.name,
            current: context! { version: p.current.version },
            creator_user: creator_user,
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
async fn partial_home_matches(api_client: &State<ApiClient>) -> Result<Template, ApiErrors> {
    let mut matches = api_client.get_matches(None, None).await;

    matches.sort_by_key(|m| -m.created_at);

    let matches_ctx: Vec<_> = join_all(matches.into_iter().map(async |m| {
        let team_a_name = api_client.get_cached_pilot_name(&m.team_a.aip_id.to_string()).await.unwrap_or(m.team_a.aip_id.to_string());
        let team_b_name = api_client.get_cached_pilot_name(&m.team_b.aip_id.to_string()).await.unwrap_or(m.team_b.aip_id.to_string());

        let download_url = if let Some(replay_id) = m.replay_id {
            Some(format!("{}/replay?replayId={}", api_client.base_url(), replay_id))
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

#[get("/login?<next>")]
async fn login(next: Option<&str>, sso_client: &State<SSOClient>) -> Result<Redirect, ApiErrors> {
    Ok(Redirect::to(sso_client.get_redirect_url(next)))
}

#[get("/login_callback?<code>")]
async fn login_callback(
    code: &str,
    cookies: &CookieJar<'_>,
    client: &State<SqliteClient>,
    sso_client: &State<SSOClient>,
) -> Result<Redirect, ApiErrors> {
    login_callback_next(None, code, cookies, client, sso_client).await
}

#[get("/login_callback/<next>?<code>")]
async fn login_callback_next(
    next: Option<&str>,
    code: &str,
    cookies: &CookieJar<'_>,
    client: &State<SqliteClient>,
    sso_client: &State<SSOClient>,
) -> Result<Redirect, ApiErrors> {
    let Some(user) = sso_client.get_user_oauth(code).await else {
        return Err(ApiErrors::BadRequest("Invalid OAuth code".into()));
    };

    let user = User::upsert_by_discord_id(&user.id, &user.username, &user.avatar, &**client)
        .await
        .map_err(|e| {
            log::error!("Failed to upsert user: {}", e);
            ApiErrors::InternalError("Failed to upsert user".into())
        })?;

    let cookie_str = serde_json::to_string(&ApiUser {
        id: user.id,
        discord_id: user.discord_id,
        username: user.username,
        avatar: user.avatar_url,
    })
    .map_err(|e| {
        log::error!("Failed to serialize user data: {}", e);
        ApiErrors::InternalError("Failed to serialize user data".into())
    })?;

    cookies.add_private(Cookie::new("auth", cookie_str));

    // Needed since cookies are queued for redirects
    let callback_redirect = format!("/login_callback_redirect?next={}", next.unwrap_or("/"));
    Ok(Redirect::found(callback_redirect))
}

#[get("/logout")]
async fn logout(cookies: &CookieJar<'_>) -> Template {
    cookies.remove_private("auth");
    Template::render("logout_callback", context! { next: "/" })
}

#[get("/login_callback_redirect?<next>")]
async fn login_callback_redirect_page(next: Option<&str>) -> Template {
    Template::render("login_callback", context! { next })
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
            build_info: build_info_ctx()
        },
    ))
}

#[get("/upload?<name>")]
async fn upload_page(
    user: ApiUser,
    name: Option<String>,
    api_client: &State<ApiClient>,
) -> Result<Template, ApiErrors> {
    let pilots = api_client.get_pilots().await;

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
            user: user,
            build_info: build_info_ctx()
        },
    ))
}

#[get("/match/create")]
async fn match_create_page(user: ApiUser) -> Result<Template, ApiErrors> {
    Ok(Template::render(
        "match_create",
        context! {
            user: user,
            build_info: build_info_ctx()
        },
    ))
}

#[get("/pilot/<pilot_name>")]
async fn pilot_stats_page(
    user: Option<ApiUser>,
    pilot_name: &str,
    sso_client: &State<SSOClient>,
    api_client: &State<ApiClient>,
) -> Result<Template, ApiErrors> {
    let pilot = api_client
        .get_pilot_by_name(pilot_name)
        .await
        .ok_or_else(|| ApiErrors::NotFound("Pilot not found".into()))?;
    let matches = api_client
        .get_matches(Some(pilot.id.to_string().as_str()), None)
        .await;

    // Calculate overall stats
    let total_matches = matches.len();
    let wins = matches
        .iter()
        .filter(|m| {
            (m.team_a.aip_id == pilot.id && m.winner == Winner::TeamA)
                || (m.team_b.aip_id == pilot.id && m.winner == Winner::TeamB)
        })
        .count();
    let losses = total_matches - wins;
    let win_rate = if total_matches > 0 {
        wins as f32 / total_matches as f32 * 100.0
    } else {
        0.0
    };

    // Group matches by opponent
    let mut opponent_stats = std::collections::HashMap::new();
    for m in &matches {
        let (opponent_id, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.winner == Winner::TeamB)
        };

        let opponent_name = api_client
            .get_cached_pilot_name(&opponent_id.to_string())
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
    let mut opponents: Vec<_> = opponent_stats
        .into_iter()
        .map(|(name, (wins, losses, total))| {
            (
                name,
                wins,
                losses,
                total,
                if total > 0 {
                    wins as f32 / total as f32 * 100.0
                } else {
                    0.0
                },
            )
        })
        .collect();
    opponents.sort_by(|a, b| b.3.cmp(&a.3)); // Sort by total matches

    let opponents_ctx: Vec<_> = opponents
        .into_iter()
        .map(|(name, wins, losses, total, win_rate)| {
            context! {
                name: name,
                wins: wins,
                losses: losses,
                total: total,
                win_rate: format!("{:.0}", win_rate),
            }
        })
        .collect();

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
    let mut versions: Vec<_> = version_stats
        .into_iter()
        .map(|(version, (wins, losses, total))| {
            (
                version,
                wins,
                losses,
                total,
                if total > 0 {
                    wins as f32 / total as f32 * 100.0
                } else {
                    0.0
                },
            )
        })
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0)); // Sort by version descending

    let versions_ctx: Vec<_> = versions
        .iter()
        .enumerate()
        .map(|(index, (version, wins, losses, total, win_rate))| {
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
        })
        .collect();

    // Recent matches (last 10) - sort by created_at descending to get latest first
    let mut sorted_matches = matches.clone();
    sorted_matches.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let recent_matches = join_all(sorted_matches.iter().take(10).map(async |m| {
        let (opponent_id, opponent_version, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.team_b.version, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.team_a.version, m.winner == Winner::TeamB)
        };

        let opponent_name = api_client.get_cached_pilot_name(&opponent_id.to_string())
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
    let is_own_pilot = user
        .as_ref()
        .map(|u| pilot_owner_id == u.discord_id.to_string())
        .unwrap_or(false);

    // Get creator info from Discord cache
    let creator_info = sso_client.get_user(&pilot_owner_id).await;
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
            user: user,
            build_info: build_info_ctx()
        },
    ))
}

#[get("/pilot/<pilot_name>/version/<version>")]
async fn partial_pilot_version_stats(
    pilot_name: &str,
    version: i32,
    api_client: &State<ApiClient>,
) -> Result<Template, ApiErrors> {
    let pilot = api_client
        .get_pilot_by_name(pilot_name)
        .await
        .ok_or_else(|| ApiErrors::NotFound("Pilot not found".into()))?;

    let all_matches = api_client
        .get_matches(
            Some(pilot.id.to_string().as_str()),
            Some(version),
        )
        .await;

    let version_matches: Vec<_> = all_matches
        .into_iter()
        .filter(|m| {
            (m.team_a.aip_id == pilot.id && m.team_a.version == version)
                || (m.team_b.aip_id == pilot.id && m.team_b.version == version)
        })
        .collect();

    let mut opponent_stats = std::collections::HashMap::new();
    for m in &version_matches {
        let (opponent_id, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.winner == Winner::TeamB)
        };
        let opponent_name = api_client
            .get_cached_pilot_name(&opponent_id.to_string())
            .await;

        let stats = opponent_stats.entry(opponent_name).or_insert((0, 0, 0));
        if won {
            stats.0 += 1;
        } else {
            stats.1 += 1;
        }
        stats.2 += 1;
    }

    let mut opponents: Vec<_> = opponent_stats
        .into_iter()
        .map(|(name, (wins, losses, total))| {
            (
                name,
                wins,
                losses,
                total,
                if total > 0 {
                    wins as f32 / total as f32 * 100.0
                } else {
                    0.0
                },
            )
        })
        .collect();
    opponents.sort_by(|a, b| b.3.cmp(&a.3));

    let opponents_ctx: Vec<_> = opponents
        .into_iter()
        .map(|(name, wins, losses, total, win_rate)| {
            context! {
                name: name,
                wins: wins,
                losses: losses,
                total: total,
                win_rate: format!("{:.0}", win_rate),
            }
        })
        .collect();

    // Recent matches for this version
    let mut sorted_matches = version_matches.clone();
    sorted_matches.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let recent_matches = join_all(sorted_matches.iter().take(10).map(async |m| {
        let (opponent_id, opponent_version, won) = if m.team_a.aip_id == pilot.id {
            (m.team_b.aip_id, m.team_b.version, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.team_a.version, m.winner == Winner::TeamB)
        };

        let opponent_name = api_client.get_cached_pilot_name(&opponent_id.to_string()).await;

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
    let wins = version_matches
        .iter()
        .filter(|m| {
            (m.team_a.aip_id == pilot.id && m.winner == Winner::TeamA)
                || (m.team_b.aip_id == pilot.id && m.winner == Winner::TeamB)
        })
        .count();
    let losses = total_matches - wins;
    let win_rate = if total_matches > 0 {
        wins as f32 / total_matches as f32 * 100.0
    } else {
        0.0
    };

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

#[get("/users")]
async fn users_page(
    user: Option<ApiUser>,
    api_client: &State<ApiClient>,
    sso_client: &State<SSOClient>,
) -> Result<Template, ApiErrors> {
    // Get all pilots to extract unique owners
    let pilots = api_client.get_pilots().await;
    
    // Create a map to collect user stats
    let mut user_map: std::collections::HashMap<String, (String, Option<String>, Vec<String>, i32, f32)> = 
        std::collections::HashMap::new(); // owner_id -> (username, avatar_url, pilot_names, total_matches, win_rate)
    
    // Process each pilot to gather user information
    for pilot in &pilots {
        let owner_id = pilot.owner_id.clone();
        let pilot_name = pilot.name.clone();
        
        // Get matches for this pilot to calculate stats
        let matches = api_client.get_matches(Some(&pilot.id.to_string()), None).await;
        let pilot_total_matches = matches.len();
        let pilot_wins = matches.iter().filter(|m| {
            (m.team_a.aip_id == pilot.id && m.winner == Winner::TeamA) ||
            (m.team_b.aip_id == pilot.id && m.winner == Winner::TeamB)
        }).count();
        
        // Get username from Discord cache
        let user_info = sso_client.get_user(&owner_id).await;
        let username = user_info.as_ref()
            .map(|u| u.username.clone())
            .unwrap_or_else(|| owner_id.clone());
        let avatar_url = user_info.as_ref()
            .map(|info| discord_avatar_url(&owner_id, &info.avatar));
        
        // Update or insert user stats
        let entry = user_map.entry(owner_id.clone()).or_insert((username, avatar_url, Vec::new(), 0, 0.0));
        entry.2.push(pilot_name);
        entry.3 += pilot_total_matches as i32;
        
        // Recalculate overall win rate (weighted average)
        if entry.3 > 0 {
            let total_wins = (entry.4 / 100.0 * (entry.3 - pilot_total_matches as i32) as f32) + pilot_wins as f32;
            entry.4 = total_wins / entry.3 as f32 * 100.0;
        }
    }
    
    // Convert to vector with struct for easier sorting
    let mut users: Vec<_> = user_map.into_iter()
        .map(|(owner_id, (username, avatar_url, pilot_names, total_matches, win_rate))| {
            (owner_id, username, avatar_url, pilot_names.len(), pilot_names, total_matches, win_rate)
        })
        .collect();
    
    // Sort by pilot count descending, then by total matches
    users.sort_by(|a, b| {
        let pilot_count_cmp = b.3.cmp(&a.3); // pilot count
        if pilot_count_cmp == std::cmp::Ordering::Equal {
            b.5.cmp(&a.5) // total matches
        } else {
            pilot_count_cmp
        }
    });
    
    // Convert to context objects
    let users_ctx: Vec<_> = users.into_iter()
        .map(|(owner_id, username, avatar_url, pilot_count, pilot_names, total_matches, win_rate)| {
            context! {
                owner_id: owner_id,
                username: username,
                avatar_url: avatar_url,
                pilot_count: pilot_count,
                pilot_names: pilot_names,
                total_matches: total_matches,
                win_rate: format!("{:.1}", win_rate),
            }
        })
        .collect();
    
    Ok(Template::render(
        "users",
        context! {
            users: users_ctx,
            user: user,
            build_info: build_info_ctx()
        },
    ))
}

#[get("/user/<owner_id>")]
async fn user_page(
    user: Option<ApiUser>,
    owner_id: &str,
    api_client: &State<ApiClient>,
    sso_client: &State<SSOClient>,
) -> Result<Template, ApiErrors> {
    // Get all pilots for this user
    let all_pilots = api_client.get_pilots().await;
    let user_pilots: Vec<_> = all_pilots.into_iter()
        .filter(|p| p.owner_id == owner_id)
        .collect();
    
    if user_pilots.is_empty() {
        return Err(ApiErrors::NotFound("User not found or has no pilots".into()));
    }
    
    // Get user info from Discord cache
    let user_info = sso_client.get_user(owner_id).await;
    let username = user_info.as_ref()
        .map(|info| info.username.clone())
        .unwrap_or_else(|| owner_id.to_string());
    let user_avatar = user_info.as_ref()
        .map(|info| discord_avatar_url(owner_id, &info.avatar));
    
    // Gather all matches for user's pilots
    let mut all_matches = Vec::new();
    let mut pilot_stats = Vec::new();
    
    for pilot in &user_pilots {
        let matches = api_client.get_matches(Some(&pilot.id.to_string()), None).await;
        all_matches.extend(matches.clone());
        
        // Calculate stats for this pilot
        let total_matches = matches.len();
        let wins = matches.iter().filter(|m| {
            (m.team_a.aip_id == pilot.id && m.winner == Winner::TeamA) ||
            (m.team_b.aip_id == pilot.id && m.winner == Winner::TeamB)
        }).count();
        let losses = total_matches - wins;
        let win_rate = if total_matches > 0 {
            wins as f32 / total_matches as f32 * 100.0
        } else {
            0.0
        };
        
        pilot_stats.push((context! {
            name: pilot.name.clone(),
            current_version: pilot.current.version,
            total_matches: total_matches,
            wins: wins,
            losses: losses,
            win_rate: format!("{:.1}", win_rate),
        }, total_matches));
    }
    
    // Sort pilots by total matches descending
    pilot_stats.sort_by(|a, b| b.1.cmp(&a.1));
    
    // Convert to context objects
    let pilot_stats: Vec<_> = pilot_stats.into_iter().map(|(ctx, _)| ctx).collect();
    
    // Calculate overall user stats
    let total_matches = all_matches.len();
    let total_wins = all_matches.iter().filter(|m| {
        user_pilots.iter().any(|pilot| {
            (m.team_a.aip_id == pilot.id && m.winner == Winner::TeamA) ||
            (m.team_b.aip_id == pilot.id && m.winner == Winner::TeamB)
        })
    }).count();
    let total_losses = total_matches - total_wins;
    let overall_win_rate = if total_matches > 0 {
        total_wins as f32 / total_matches as f32 * 100.0
    } else {
        0.0
    };
    
    // Get recent matches (last 20, sorted by date)
    let mut sorted_matches = all_matches.clone();
    sorted_matches.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let recent_matches = join_all(sorted_matches.iter().take(20).map(async |m| {
        // Find which pilot was involved in this match
        let user_pilot = user_pilots.iter().find(|pilot| {
            m.team_a.aip_id == pilot.id || m.team_b.aip_id == pilot.id
        }).unwrap();
        
        let (opponent_id, opponent_version, pilot_version, won) = if m.team_a.aip_id == user_pilot.id {
            (m.team_b.aip_id, m.team_b.version, m.team_a.version, m.winner == Winner::TeamA)
        } else {
            (m.team_a.aip_id, m.team_a.version, m.team_b.version, m.winner == Winner::TeamB)
        };

        let opponent_name = api_client.get_cached_pilot_name(&opponent_id.to_string())
            .await
            .unwrap_or(opponent_id.to_string());
        
        context! {
            pilot_name: user_pilot.name.clone(),
            pilot_version: pilot_version,
            opponent: opponent_name,
            opponent_version: opponent_version,
            won: won,
            created_at: format_date_time(&chrono::DateTime::<chrono::Utc>::from_timestamp(m.created_at / 1_000, 0).unwrap_or_default()),
            is_manual: m.manual_run,
        }
    })).await;
    
    Ok(Template::render(
        "user",
        context! {
            target_user: context! {
                owner_id: owner_id,
                username: username,
                avatar: user_avatar,
            },
            overall_stats: context! {
                pilot_count: user_pilots.len(),
                total_matches: total_matches,
                wins: total_wins,
                losses: total_losses,
                win_rate: format!("{:.1}", overall_win_rate),
            },
            pilots: pilot_stats,
            recent_matches: recent_matches,
            user: user,
            build_info: build_info_ctx()
        },
    ))
}

fn render_error_page(code: u16, message: &str) -> Template {
    Template::render(
        "error",
        context! {
        code: code.to_string(),
            message: message,
            build_info: build_info_ctx(),
        },
    )
}

#[catch(401)]
fn unauthorized_catcher(_status: Status, req: &rocket::Request<'_>) -> Redirect {
    let next = req.uri().path();
    Redirect::to(format!("/login?next={}", next))
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
        _ => "Error",
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

    let sso_client = SSOClient::new();
    let api_client = ApiClient::new();

    // Pre-warm cache
    spawn({
        let api_client = api_client.clone();
        async move {
            let _ = api_client.get_pilots().await;
            let _ = api_client.get_matches(None, None).await;
        }
    });

    rocket::build()
        .manage(client)
        .manage(sso_client)
        .manage(api_client)
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
                match_create_page,
                pilot_stats_page,
                partial_pilot_version_stats,
                users_page,
                user_page,
                login_callback_redirect_page,
                login,
                login_callback,
                login_callback_next,
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
        .register(
            "/",
            catchers![
                unauthorized_catcher,
                not_found_catcher,
                internal_error_catcher,
                default_catcher
            ],
        )
        .attach(Template::fairing())
}

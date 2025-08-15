use std::{env, time::Duration};

use log::error;
use moka::future::Cache;
use serde::Deserialize;

use crate::model::User;

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordUserInfo {
    pub id: String,
    pub username: String,
    pub avatar: String,
}

pub struct SSOClient {
    client: reqwest::Client,
    cache: Cache<String, DiscordUserInfo>,
    own_base_url: String,
}

impl SSOClient {
    pub fn new() -> Self {
        let cache = Cache::builder()
            .max_capacity(2048)
            .time_to_live(Duration::from_secs(60 * 60 * 24))
            .build();
        let client = reqwest::Client::new();
        let own_base_url = env::var("BASE_URL").expect("BASE_URL must be set");

        SSOClient {
            client,
            cache,
            own_base_url,
        }
    }

    pub async fn get_user(&self, discord_id: &str) -> Option<DiscordUserInfo> {
        self.cache
            .optionally_get_with(discord_id.to_string(), self.fetch_discord_user(discord_id))
            .await
    }

    async fn fetch_discord_user(&self, discord_id: &str) -> Option<DiscordUserInfo> {
        self.client
            .get(format!("https://sso.isan.to/uinfo/{}", discord_id))
            .send()
            .await
            .ok()?
            .json::<DiscordUserInfo>()
            .await
            .ok()
    }

    pub fn get_redirect_url(&self) -> String {
        format!(
            "https://sso.isan.to/login?service={}/login_callback",
            self.own_base_url
        )
    }

    pub async fn get_user_oauth(&self, code: &str) -> Option<DiscordUserInfo> {
        let res = self
            .client
            .get(format!("https://sso.isan.to/getuser/{}", code))
            .send()
            .await;

        let res = match res {
            Ok(response) => response,
            Err(e) => {
                error!("Failed to get user data: {}", e);
                return None;
            }
        };

        let data = match res.json::<DiscordUserInfo>().await {
            Ok(user_info) => user_info,
            Err(e) => {
                error!("Failed to parse user data: {}", e);
                return None;
            }
        };

        Some(data)
    }
}

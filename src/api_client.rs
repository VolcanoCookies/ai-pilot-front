use std::env;

use client::{
    apis::configuration::{ApiKey, Configuration},
    models::{AiPilot, MatchResult},
};
use moka::future::Cache;
use rocket::futures::future::join_all;
use uuid::Uuid;
pub struct ApiClient {
    configuration: Configuration,
    pilot_name_cache: Cache<String, String>,
}

impl ApiClient {
    pub fn new() -> Self {
        let configuration = Configuration {
            base_path: env::var("AIP_API_BASE_URL").expect("AIP_API_BASE_URL must be set"),
            user_agent: Some("api-front/1.0".to_string()),
            api_key: Some(ApiKey {
                prefix: None,
                key: env::var("AIP_API_KEY").expect("AIP_API_KEY must be set"),
            }),
            ..Default::default()
        };

        let pilot_name_cache = Cache::builder().max_capacity(2048).build();

        ApiClient {
            configuration,
            pilot_name_cache,
        }
    }

    pub async fn get_match(&self, match_id: &str) -> Option<MatchResult> {
        match client::apis::default_api::get_match_results(
            &self.configuration,
            None,
            None,
            Some(match_id),
        )
        .await
        {
            Ok(mut matches) => matches.pop(),
            Err(e) => {
                error!("Failed to fetch match result: {}", e);
                None
            }
        }
    }

    pub async fn get_matches(
        &self,
        pilot_id: Option<&str>,
        pilot_version: Option<i32>,
    ) -> Vec<MatchResult> {
        match client::apis::default_api::get_match_results(
            &self.configuration,
            pilot_id,
            pilot_version.map(|v| v.to_string()).as_deref(),
            None,
        )
        .await
        {
            Ok(matches) => matches,
            Err(e) => {
                error!("Failed to fetch match results: {}", e);
                Vec::new()
            }
        }
    }

    pub async fn get_pilot(&self, pilot_id: &str) -> Option<AiPilot> {
        match client::apis::default_api::get_ai_pilots(&self.configuration, None, Some(pilot_id))
            .await
        {
            Ok(mut pilots) => pilots.pop(),
            Err(e) => {
                error!("Failed to fetch pilot: {}", e);
                None
            }
        }
    }

    pub async fn get_pilot_by_name(&self, pilot_name: &str) -> Option<AiPilot> {
        match client::apis::default_api::get_ai_pilots(&self.configuration, Some(pilot_name), None)
            .await
        {
            Ok(mut pilots) => pilots.pop(),
            Err(e) => {
                error!("Failed to fetch pilot by name: {}", e);
                None
            }
        }
    }

    pub async fn get_pilots(&self) -> Vec<AiPilot> {
        match client::apis::default_api::get_ai_pilots(&self.configuration, None, None).await {
            Ok(pilots) => {
                join_all(pilots.iter().map(|pilot| {
                    self.pilot_name_cache
                        .insert(pilot.id.to_string(), pilot.name.clone())
                }))
                .await;
                pilots
            }
            Err(e) => {
                error!("Failed to fetch pilot list: {}", e);
                Vec::new()
            }
        }
    }

    pub async fn upload_ai_pilot(
        &self,
        name: &str,
        owner: &str,
        data: Vec<u8>,
    ) -> Result<(Uuid, i32), String> {
        let res = client::apis::default_api::upload_ai_pilot(
            &self.configuration,
            name,
            data,
            Some(owner),
        )
        .await
        .map_err(|e| e.to_string())?;

        Ok((res.upload_id, res.version))
    }

    pub async fn get_cached_pilot_name(&self, pilot_id: &str) -> Option<String> {
        self.pilot_name_cache.get(pilot_id).await
    }

    pub fn base_url(&self) -> &str {
        &self.configuration.base_path
    }
}

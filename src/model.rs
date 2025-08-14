use chrono::{DateTime, Utc};
use rocket::serde::{Deserialize, Serialize};

use sqlx::prelude::FromRow;

use crate::{SqliteClient, api_error::ApiErrors};

pub type UserId = i64;
pub type UserTokenId = i64;

#[derive(Debug, Serialize, Deserialize, JsonSchema, FromRow)]
pub struct User {
    pub id: UserId,
    pub discord_id: String,
    pub username: String,
    pub avatar_url: String,
}

impl User {
    pub async fn upsert_by_discord_id(
        discord_id: &str,
        username: &str,
        avatar_url: &str,
        client: &SqliteClient,
    ) -> Result<User, sqlx::Error> {
        let res = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (discord_id, username, avatar_url)
            VALUES ($1, $2, $3)
            ON CONFLICT (discord_id) DO UPDATE SET username = EXCLUDED.username, avatar_url = EXCLUDED.avatar_url
            RETURNING id, discord_id, username, avatar_url
            "#,
        )
        .bind(discord_id)
        .bind(username)
        .bind(avatar_url)
        .fetch_one(client)
        .await?;

        Ok(res)
    }

    pub async fn all(client: &SqliteClient) -> Result<Vec<User>, sqlx::Error> {
        let res = sqlx::query_as::<_, User>(
            r#"
            SELECT id, discord_id, username, avatar_url
            FROM users
            "#,
        )
        .fetch_all(client)
        .await?;

        Ok(res)
    }

    pub async fn get_by_id(id: UserId, client: &SqliteClient) -> Result<User, sqlx::Error> {
        let res = sqlx::query_as::<_, User>(
            r#"
            SELECT id, discord_id, username, avatar_url
            FROM users
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_one(client)
        .await?;

        Ok(res)
    }

    pub async fn get_user_by_user_token(
        token: &str,
        client: &SqliteClient,
    ) -> Result<User, ApiErrors> {
        let res = sqlx::query_as::<_, User>(
            r#"
            SELECT users.id, users.discord_id, users.username, users.avatar_url
            FROM users
            INNER JOIN user_tokens ON users.id = user_tokens.user_id
            WHERE user_tokens.token = $1 AND (user_tokens.expires_at > $2 OR user_tokens.expires_at IS NULL)
            "#,
        )
        .bind(token)
        .bind(Utc::now())
        .fetch_one(client)
        .await
        .map_err(|e| {
            log::error!("Failed to fetch user by token: {}", e);
            ApiErrors::InternalError("Failed to fetch user by token".into())
        })?;

        Ok(res)
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, FromRow)]
pub struct UserToken {
    pub id: UserTokenId,
    pub name: String,
    pub user_id: UserId,
    pub token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl UserToken {
    pub async fn insert_user_token(
        name: String,
        user_id: UserId,
        expires_at: Option<DateTime<Utc>>,
        client: &SqliteClient,
    ) -> Result<UserToken, sqlx::Error> {
        let res = sqlx::query_as::<_, UserToken>(
            r#"
            INSERT INTO user_tokens (name, user_id, token, created_at, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, name, user_id, token, created_at, expires_at
            "#,
        )
        .bind(name)
        .bind(user_id)
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(Utc::now())
        .bind(expires_at)
        .fetch_one(client)
        .await?;

        Ok(res)
    }

    pub async fn get_by_user_id(
        user_id: UserId,
        client: &SqliteClient,
    ) -> Result<Vec<UserToken>, ApiErrors> {
        let res = sqlx::query_as::<_, UserToken>(
            r#"
            SELECT id, name, user_id, token, created_at, expires_at
            FROM user_tokens
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_all(client)
        .await
        .map_err(|e| {
            log::error!("Failed to fetch user token: {}", e);
            ApiErrors::InternalError("Failed to fetch user token".into())
        })?;

        Ok(res)
    }

    pub async fn get_by_token(token: &str, client: &SqliteClient) -> Result<UserToken, ApiErrors> {
        let res = sqlx::query_as::<_, UserToken>(
            r#"
            SELECT id, name, user_id, token, created_at, expires_at
            FROM user_tokens
            WHERE token = $1
            "#,
        )
        .bind(token)
        .fetch_one(client)
        .await
        .map_err(|e| {
            log::error!("Failed to fetch user token: {}", e);
            ApiErrors::InternalError("Failed to fetch user token".into())
        })?;

        Ok(res)
    }

    pub async fn delete_by_id_and_user_id(
        id: UserTokenId,
        user_id: UserId,
        client: &SqliteClient,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            DELETE FROM user_tokens
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(id)
        .bind(user_id)
        .execute(client)
        .await?;

        Ok(())
    }
}

pub trait ResultExt<T, E> {
    fn or_not_found(self, entity_name: &str) -> Result<T, ApiErrors>;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn or_not_found(self, entity_name: &str) -> Result<T, ApiErrors> {
        self.map_err(|_| ApiErrors::NotFound(format!("{} not found", entity_name).into()))
    }
}

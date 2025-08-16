use okapi::openapi3::{Object, Parameter};
use rocket::{
    Request, State,
    http::Status,
    request::{FromRequest, Outcome},
};
use rocket_okapi::{r#gen::OpenApiGenerator, request::OpenApiFromRequest};
use serde::{Deserialize, Serialize};

use crate::{
    SqliteClient,
    model::{User, UserId},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiUser {
    pub id: UserId,
    pub discord_id: String,
    pub username: String,
    pub avatar: String,
}

#[async_trait]
impl<'r> FromRequest<'r> for ApiUser {
    type Error = String;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        if let Some(cookie) = request.cookies().get_private("auth") {
            if let Ok(user) = serde_json::from_str(cookie.value()) {
                return Outcome::Success(user);
            } else {
                return Outcome::Error((Status::Unauthorized, "Malformed auth cookie".to_string()));
            }
        } else if let Some(auth_token) = request.headers().get_one("x-auth-token") {
            if let Outcome::Success(client) = request.guard::<&State<SqliteClient>>().await {
                if let Ok(user) = User::get_user_by_user_token(auth_token, client).await {
                    return Outcome::Success(ApiUser {
                        id: user.id,
                        discord_id: user.discord_id,
                        username: user.username,
                        avatar: user.avatar_url,
                    });
                }
            }
        }

        return Outcome::Error((Status::Unauthorized, "Auth cookie missing".to_string()));
    }
}

impl<'a> OpenApiFromRequest<'a> for ApiUser {
    fn from_request_input(
        gene: &mut OpenApiGenerator,
        _name: String,
        required: bool,
    ) -> rocket_okapi::Result<rocket_okapi::request::RequestHeaderInput> {
        let schema = gene.json_schema::<String>();

        Ok(rocket_okapi::request::RequestHeaderInput::Parameter(
            Parameter {
                name: "auth".to_owned(),
                location: "cookie".to_owned(),
                description: None,
                required,
                deprecated: false,
                allow_empty_value: false,
                value: rocket_okapi::okapi::openapi3::ParameterValue::Schema {
                    style: None,
                    explode: None,
                    allow_reserved: false,
                    schema,
                    example: None,
                    examples: None,
                },
                extensions: Object::default(),
            },
        ))
    }
}

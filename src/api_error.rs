use okapi::{Map, openapi3::RefOr};
use rocket::{Request, http::Status, response::Responder, serde::json::Json};
use rocket_dyn_templates::{Template, context};
use rocket_okapi::{JsonSchema, r#gen::OpenApiGenerator, response::OpenApiResponderInner};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
struct ErrorMessageInner {
    message: String,
}

#[derive(Debug)]
pub enum ApiErrors {
    NotFound(String),
    BadRequest(String),
    InternalError(String),
}

impl ApiErrors {
    pub fn status_code(&self) -> u16 {
        match self {
            ApiErrors::NotFound(_) => 404,
            ApiErrors::BadRequest(_) => 400,
            ApiErrors::InternalError(_) => 500,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ApiErrors::NotFound(msg) => msg,
            ApiErrors::BadRequest(msg) => msg,
            ApiErrors::InternalError(msg) => msg,
        }
    }

    pub fn default_message(&self) -> &str {
        match self {
            ApiErrors::NotFound(_) => "Not Found",
            ApiErrors::BadRequest(_) => "Bad Request",
            ApiErrors::InternalError(_) => "Internal Server Error",
        }
    }
}

impl From<&str> for ApiErrors {
    fn from(message: &str) -> Self {
        ApiErrors::InternalError(message.to_string())
    }
}

impl From<String> for ApiErrors {
    fn from(message: String) -> Self {
        ApiErrors::InternalError(message)
    }
}

impl<'r> Responder<'r, 'static> for ApiErrors {
    fn respond_to(self, request: &'r Request<'_>) -> rocket::response::Result<'static> {
        let accepts_html = request
            .headers()
            .get("Accept")
            .any(|accept| accept.contains("text/html"));

        if accepts_html {
            // Render HTML error page
            let template = Template::render(
                "error",
                context! {
                    code: self.status_code().to_string(),
                    message: self.message()
                },
            );
            template.respond_to(request)
        } else {
            // Render JSON error
            let json_response = Json(ErrorMessageInner {
                message: self.message().to_string(),
            });

            let mut response = json_response.respond_to(request)?;
            response.set_status(Status::from_code(self.status_code()).unwrap());
            Ok(response)
        }
    }
}

impl OpenApiResponderInner for ApiErrors {
    fn responses(gene: &mut OpenApiGenerator) -> rocket_okapi::Result<okapi::openapi3::Responses> {
        let mut responses = Map::new();

        responses.insert(
            "404".to_string(),
            RefOr::Object(okapi::openapi3::Response {
                description: "Not Found".to_string(),
                content: Map::from([(
                    "application/json".to_string(),
                    okapi::openapi3::MediaType {
                        schema: Some(gene.json_schema::<ErrorMessageInner>()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        );

        responses.insert(
            "400".to_string(),
            RefOr::Object(okapi::openapi3::Response {
                description: "Bad Request".to_string(),
                content: Map::from([(
                    "application/json".to_string(),
                    okapi::openapi3::MediaType {
                        schema: Some(gene.json_schema::<ErrorMessageInner>()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        );

        responses.insert(
            "500".to_string(),
            RefOr::Object(okapi::openapi3::Response {
                description: "Internal Server Error".to_string(),
                content: Map::from([(
                    "application/json".to_string(),
                    okapi::openapi3::MediaType {
                        schema: Some(gene.json_schema::<ErrorMessageInner>()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        );

        Ok(okapi::openapi3::Responses {
            responses,
            ..Default::default()
        })
    }
}

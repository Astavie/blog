use std::net::SocketAddr;

use activitypub_federation::{
    axum::json::FederationJson,
    config::{Data, FederationConfig, FederationMiddleware},
    fetch::webfinger::{build_webfinger_response, extract_webfinger_name, Webfinger},
    kinds::{
        activity::CreateType, actor::PersonType, collection::OrderedCollectionType,
        object::NoteType, public,
    },
    protocol::context::WithContext,
};
use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone)]
struct Blog {
    hostname: String,
    authors: Vec<Author>,
    posts: Vec<Post>,
}

#[derive(Clone)]
struct Post {
    author: String,
    published: DateTime<Utc>,
    title: String,
    content: String,
}

#[derive(Debug)]
enum Error {
    Internal(anyhow::Error),
    NotFound,
}

impl<T> From<T> for Error
where
    T: Into<anyhow::Error>,
{
    fn from(t: T) -> Self {
        Error::Internal(t.into())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Error::Internal(err) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", err)).into_response()
            }
            Error::NotFound => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        }
    }
}

#[derive(Debug, Clone)]
struct Author {
    name: String,
    display_name: String,
    followers: Vec<Url>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Note {
    #[serde(rename = "type")]
    kind: NoteType,
    id: Url,
    published: String,
    url: Url,
    to: Vec<Url>,
    cc: Vec<Url>,
    content: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Create {
    #[serde(rename = "type")]
    kind: CreateType,
    id: Url,
    published: String,
    to: Vec<Url>,
    cc: Vec<Url>,
    object: Note,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Person {
    id: Url,
    #[serde(rename = "type")]
    kind: PersonType,
    preferred_username: String,
    name: String,
    inbox: Url,
    outbox: Url,
    following: Url,
    followers: Url,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderedCollection<T> {
    #[serde(rename = "type")]
    kind: OrderedCollectionType,
    total_items: usize,
    ordered_items: Vec<T>,
}

impl Post {
    fn into_json(&self, data: &Data<Blog>) -> Result<Create, Error> {
        let published = self.published.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let to = vec![Url::parse(&format!(
            "{}/users/{}/followers",
            data.hostname, self.author
        ))?];
        let cc = vec![public()];

        Ok(Create {
            kind: CreateType::Create,
            id: Url::parse(&format!(
                "{}/users/{}/statuses/{}/activity",
                data.hostname,
                self.author,
                self.published.timestamp()
            ))?,
            published: published.clone(),
            to: to.clone(),
            cc: cc.clone(),
            object: Note {
                kind: NoteType::Note,
                id: Url::parse(&format!(
                    "{}/users/{}/statuses/{}",
                    data.hostname,
                    self.author,
                    self.published.timestamp()
                ))?,
                published,
                url: Url::parse(&format!(
                    "{}/blog/{}",
                    data.hostname,
                    self.title.to_lowercase().replace(' ', "-")
                ))?,
                to,
                cc,
                content: format!("{}\n---\n\n{}", self.title, self.content),
            },
        })
    }
}

impl Author {
    fn into_json(&self, data: &Data<Blog>) -> Result<Person, Error> {
        Ok(Person {
            kind: PersonType::Person,
            id: Url::parse(&format!("{}/users/{}", data.hostname, self.name))?,
            inbox: Url::parse(&format!("{}/users/{}/inbox", data.hostname, self.name))?,
            outbox: Url::parse(&format!("{}/users/{}/outbox", data.hostname, self.name))?,
            following: Url::parse(&format!("{}/users/{}/following", data.hostname, self.name))?,
            followers: Url::parse(&format!("{}/users/{}/followers", data.hostname, self.name))?,
            preferred_username: self.name.clone(),
            name: self.display_name.clone(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let hostname = if cfg!(debug_assertions) {
        "http://localhost:3000"
    } else {
        "https://astavie.dev"
    };

    let domain = if cfg!(debug_assertions) {
        "localhost:3000"
    } else {
        "astavie.dev"
    };

    let blog = Blog {
        hostname: hostname.into(),
        authors: vec![Author {
            name: "astavie".into(),
            display_name: "Astavie".into(),
            followers: vec![],
        }],
        posts: vec![Post {
            author: "astavie".into(),
            published: Utc::now(),
            title: "Initial post".into(),
            content: "Hello, Fediverse!".into(),
        }],
    };

    let data = FederationConfig::builder()
        .domain(domain)
        .app_data(blog)
        .build()
        .await?;

    let app = axum::Router::new()
        .route("/users/:name", get(http_get_user))
        .route("/users/:name/outbox", get(http_get_outbox))
        .route("/.well-known/webfinger", get(webfinger))
        .layer(FederationMiddleware::new(data));

    let addr = SocketAddr::from(([0, 0, 0, 0], 80));
    tracing::debug!("listening on {}", addr);

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn http_get_user(
    Path(name): Path<String>,
    data: Data<Blog>,
) -> Result<FederationJson<WithContext<Person>>, Error> {
    let user = data
        .authors
        .iter()
        .find(|a| a.name == name)
        .ok_or(Error::NotFound)?;
    let person = user.into_json(&data)?;
    Ok(FederationJson(WithContext::new_default(person)))
}

async fn http_get_outbox(
    Path(name): Path<String>,
    data: Data<Blog>,
) -> Result<FederationJson<WithContext<OrderedCollection<Create>>>, Error> {
    let _user = data
        .authors
        .iter()
        .find(|a| a.name == name)
        .ok_or(Error::NotFound)?;
    let posts = data
        .posts
        .iter()
        .filter(|p| p.author == name)
        .map(|p| p.into_json(&data))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FederationJson(WithContext::new_default(
        OrderedCollection {
            kind: OrderedCollectionType::OrderedCollection,
            total_items: posts.len(),
            ordered_items: posts,
        },
    )))
}

#[derive(Deserialize)]
pub struct WebfingerQuery {
    resource: String,
}

async fn webfinger(
    Query(query): Query<WebfingerQuery>,
    data: Data<Blog>,
) -> Result<Json<Webfinger>, Error> {
    let name = extract_webfinger_name(&query.resource, &data)?;
    let user = data
        .authors
        .iter()
        .find(|a| a.name == name)
        .ok_or(Error::NotFound)?;
    Ok(Json(build_webfinger_response(
        query.resource,
        user.into_json(&data)?.id,
    )))
}

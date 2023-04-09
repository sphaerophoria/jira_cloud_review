use futures::prelude::*;
use isahc::{
    auth::{Authentication, Credentials},
    prelude::*,
    Request,
};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt,
    io::{self, Read},
};
use tracing::debug;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub jql: String,
    pub start_at: i32,
    pub expand: Vec<String>,
    pub fields: Vec<String>,
    pub max_results: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueBeanFields {
    pub summary: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueBean {
    pub id: String,
    pub key: String,
    pub fields: IssueBeanFields,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub issues: Vec<IssueBean>,
    pub start_at: i32,
    pub total: i32,
}

#[derive(Debug)]
pub enum SearchError {
    SearchResponseParse(serde_json::Error),
    ReadResponse(io::Error),
    HttpRequest(isahc::http::Error),
    HttpSend(isahc::Error),
    InvalidLength(std::num::TryFromIntError),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::SearchResponseParse(_) => write!(f, "failed to parse response"),
            SearchError::ReadResponse(_) => write!(f, "failed to read response"),
            SearchError::HttpRequest(_) => write!(f, "failed to generate http request"),
            SearchError::HttpSend(_) => write!(f, "failed to send http request"),
            SearchError::InvalidLength(_) => write!(f, "invalid length in response"),
        }
    }
}

impl Error for SearchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SearchError::SearchResponseParse(e) => Some(e),
            SearchError::ReadResponse(e) => Some(e),
            SearchError::HttpRequest(e) => Some(e),
            SearchError::HttpSend(e) => Some(e),
            SearchError::InvalidLength(e) => Some(e),
        }
    }
}

pub fn execute_search(
    uri: &str,
    credentials: &Credentials,
    jql: String,
) -> Result<Vec<IssueBean>, SearchError> {
    let uri = format!("{uri}/rest/api/3/search");
    let start_at = 0;
    let max_results = i32::MAX;
    let expand = ["renderedBody"].into_iter().map(str::to_string).collect();
    let fields = ["summary", "comment"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let mut request = SearchRequest {
        jql,
        max_results,
        start_at,
        fields,
        expand,
    };

    let mut issues = Vec::new();

    loop {
        debug!("Issuing search to {uri}: {request:?}");

        let request_body = serde_json::to_string(&request).expect("Failed to serialize request");
        let mut response = Request::post(&uri)
            .credentials(credentials.clone())
            .authentication(Authentication::basic())
            .header("Content-Type", "application/json")
            .body(request_body)
            .map_err(SearchError::HttpRequest)?
            .send()
            .map_err(SearchError::HttpSend)?;

        let mut body = String::new();
        response
            .body_mut()
            .read_to_string(&mut body)
            .map_err(SearchError::ReadResponse)?;

        debug!("response: {}", body);

        let response_data: SearchResponse =
            serde_json::from_str(&body).map_err(SearchError::SearchResponseParse)?;

        let response_len: i32 = response_data
            .issues
            .len()
            .try_into()
            .map_err(SearchError::InvalidLength)?;

        let done = response_data.start_at + response_len == response_data.total;

        request.start_at += response_len;

        issues.extend(response_data.issues);

        if done {
            break;
        }
    }

    Ok(issues)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDetails {
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub created: String,
    pub updated: String,
    pub author: UserDetails,
    pub rendered_body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetCommentsResponse {
    comments: Vec<Comment>,
    start_at: i64,
    total: i64,
}

#[derive(Debug)]
pub enum GetCommentError {
    HttpRequest(isahc::http::Error),
    HttpSend(isahc::Error),
    ReadResponse(io::Error),
    GetCommentResponseParse(serde_json::Error),
    InvalidLength(std::num::TryFromIntError),
}

impl fmt::Display for GetCommentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GetCommentError::HttpRequest(_) => write!(f, "failed to generate http request"),
            GetCommentError::HttpSend(_) => write!(f, "failed to send http request"),
            GetCommentError::ReadResponse(_) => write!(f, "failed to read http response"),
            GetCommentError::GetCommentResponseParse(_) => write!(f, "failed to parse response"),
            GetCommentError::InvalidLength(_) => write!(f, "invalid length"),
        }
    }
}

impl Error for GetCommentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            GetCommentError::HttpRequest(e) => Some(e),
            GetCommentError::HttpSend(e) => Some(e),
            GetCommentError::ReadResponse(e) => Some(e),
            GetCommentError::GetCommentResponseParse(e) => Some(e),
            GetCommentError::InvalidLength(e) => Some(e),
        }
    }
}

pub async fn get_comments(
    uri: &str,
    credentials: &Credentials,
    issue_key: String,
) -> Result<Vec<Comment>, GetCommentError> {
    let mut start_at = 0i64;
    let max_results = i32::MAX;

    let mut comments = Vec::new();

    loop {
        let uri = format!("{uri}/rest/api/3/issue/{issue_key}/comment?expand=renderedBody&startAt={start_at}&maxResults={max_results}");

        debug!("Retrieving comments at {uri}");

        let mut response = Request::get(&uri)
            .credentials(credentials.clone())
            .authentication(Authentication::basic())
            .body(())
            .map_err(GetCommentError::HttpRequest)?
            .send_async()
            .await
            .map_err(GetCommentError::HttpSend)?;

        let mut body = String::new();
        response
            .body_mut()
            .read_to_string(&mut body)
            .await
            .map_err(GetCommentError::ReadResponse)?;

        debug!("response: {}", body);

        let response_data: GetCommentsResponse =
            serde_json::from_str(&body).map_err(GetCommentError::GetCommentResponseParse)?;

        let response_len: i64 = response_data
            .comments
            .len()
            .try_into()
            .map_err(GetCommentError::InvalidLength)?;

        let done = response_data.start_at + response_len == response_data.total;

        start_at += response_len;

        comments.extend(response_data.comments);

        if done {
            break;
        }
    }

    Ok(comments)
}

use futures::{future::BoxFuture, prelude::*};
use isahc::{
    auth::{Authentication, Credentials},
    prelude::*,
    HttpClient, Request,
};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt, fs,
    io::{self, Read},
    path::PathBuf,
};
use tracing::debug;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub jql: String,
    pub start_at: i32,
    pub expand: Vec<String>,
    pub fields: Vec<String>,
    pub max_results: i32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueBeanFields {
    pub summary: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueBean {
    pub id: String,
    pub key: String,
    pub fields: IssueBeanFields,
}

#[derive(Serialize, Deserialize, Debug)]
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
    CreateRecorder(io::Error),
    RecordBody(io::Error),
    ReadReplayDir(io::Error),
    ReadReplayFile(io::Error),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::SearchResponseParse(_) => write!(f, "failed to parse response"),
            SearchError::ReadResponse(_) => write!(f, "failed to read response"),
            SearchError::HttpRequest(_) => write!(f, "failed to generate http request"),
            SearchError::HttpSend(_) => write!(f, "failed to send http request"),
            SearchError::InvalidLength(_) => write!(f, "invalid length in response"),
            SearchError::CreateRecorder(_) => write!(f, "failed to create recorder"),
            SearchError::RecordBody(_) => write!(f, "failed to record response"),
            SearchError::ReadReplayDir(_) => write!(f, "failed to read replay dir"),
            SearchError::ReadReplayFile(_) => write!(f, "failed to read replay file"),
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
            SearchError::CreateRecorder(e) => Some(e),
            SearchError::RecordBody(e) => Some(e),
            SearchError::ReadReplayDir(e) => Some(e),
            SearchError::ReadReplayFile(e) => Some(e),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDetails {
    pub display_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub created: String,
    pub updated: String,
    pub author: UserDetails,
    pub rendered_body: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetCommentsResponse {
    comments: Vec<Comment>,
    start_at: i64,
    total: i64,
}

#[derive(Debug)]
pub enum GetCommentError {
    HttpSend(isahc::Error),
    ReadResponse(io::Error),
    GetCommentResponseParse(serde_json::Error),
    InvalidLength(std::num::TryFromIntError),
    CreateRecorder(io::Error),
    RecordBody(io::Error),
    ReadReplayDir(io::Error),
    ReadReplayFile(io::Error),
}

impl fmt::Display for GetCommentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GetCommentError::HttpSend(_) => write!(f, "failed to send http request"),
            GetCommentError::ReadResponse(_) => write!(f, "failed to read http response"),
            GetCommentError::GetCommentResponseParse(_) => write!(f, "failed to parse response"),
            GetCommentError::InvalidLength(_) => write!(f, "invalid length"),
            GetCommentError::CreateRecorder(_) => write!(f, "failed to create recorder"),
            GetCommentError::RecordBody(_) => write!(f, "failed to record response"),
            GetCommentError::ReadReplayDir(_) => write!(f, "failed to read replay dir"),
            GetCommentError::ReadReplayFile(_) => write!(f, "failed to read replay file"),
        }
    }
}

impl Error for GetCommentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            GetCommentError::HttpSend(e) => Some(e),
            GetCommentError::ReadResponse(e) => Some(e),
            GetCommentError::GetCommentResponseParse(e) => Some(e),
            GetCommentError::InvalidLength(e) => Some(e),
            GetCommentError::CreateRecorder(e) => Some(e),
            GetCommentError::RecordBody(e) => Some(e),
            GetCommentError::ReadReplayDir(e) => Some(e),
            GetCommentError::ReadReplayFile(e) => Some(e),
        }
    }
}

#[derive(Debug)]
pub enum JiraClientCreationError {
    HttpClient(isahc::Error),
    CreateRecordingDir(io::Error),
    CheckRecordingDir(io::Error),
    RecordingPathNotEmpty,
}

impl fmt::Display for JiraClientCreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JiraClientCreationError::HttpClient(_) => write!(f, "failed to construct http client"),
            JiraClientCreationError::CreateRecordingDir(_) => {
                write!(f, "failed to create recording dir")
            }
            JiraClientCreationError::CheckRecordingDir(_) => {
                write!(f, "failed to check recording dir content")
            }
            JiraClientCreationError::RecordingPathNotEmpty => {
                write!(f, "recording dir is not empty")
            }
        }
    }
}

impl Error for JiraClientCreationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            JiraClientCreationError::HttpClient(e) => Some(e),
            JiraClientCreationError::CreateRecordingDir(e) => Some(e),
            JiraClientCreationError::CheckRecordingDir(e) => Some(e),
            JiraClientCreationError::RecordingPathNotEmpty => None,
        }
    }
}

struct ApiRecorder {
    dir: PathBuf,
    counter: usize,
}

impl ApiRecorder {
    fn new(dir: PathBuf) -> Result<ApiRecorder, io::Error> {
        let counter = 0;
        fs::create_dir_all(&dir)?;
        Ok(ApiRecorder { dir, counter })
    }

    fn record_response(&mut self, body: &str) -> Result<(), io::Error> {
        let body_path = self.dir.join(self.counter.to_string());
        fs::write(body_path, body)?;
        self.counter += 1;
        Ok(())
    }
}

pub trait JiraClient {
    fn execute_search(&self, jql: String) -> Result<Vec<IssueBean>, SearchError>;
    fn get_comments(&self, issue_key: String) -> BoxFuture<Result<Vec<Comment>, GetCommentError>>;
}

pub struct HttpJiraClient<'a> {
    uri: &'a str,
    http_client: HttpClient,
    record_path: Option<PathBuf>,
}

impl<'a> HttpJiraClient<'a> {
    pub fn new(
        uri: &'a str,
        credentials: &'a Credentials,
        record_path: Option<PathBuf>,
    ) -> Result<HttpJiraClient<'a>, JiraClientCreationError> {
        let http_client = HttpClient::builder()
            .credentials(credentials.clone())
            .authentication(Authentication::basic())
            .redirect_policy(isahc::config::RedirectPolicy::Follow)
            .build()
            .map_err(JiraClientCreationError::HttpClient)?;

        // Assert record path is empty
        if let Some(record_path) = &record_path {
            fs::create_dir_all(record_path).map_err(JiraClientCreationError::CreateRecordingDir)?;

            if record_path
                .read_dir()
                .map_err(JiraClientCreationError::CheckRecordingDir)?
                .next()
                .is_some()
            {
                return Err(JiraClientCreationError::RecordingPathNotEmpty);
            }
        }

        Ok(HttpJiraClient {
            uri,
            http_client,
            record_path,
        })
    }
}

impl<'a> JiraClient for HttpJiraClient<'a> {
    fn execute_search(&self, jql: String) -> Result<Vec<IssueBean>, SearchError> {
        let uri = format!("{uri}/rest/api/3/search", uri = self.uri);
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

        let mut recorder = match &self.record_path {
            Some(p) => Some(
                ApiRecorder::new(p.join("execute_search")).map_err(SearchError::CreateRecorder)?,
            ),
            None => None,
        };

        loop {
            debug!("Issuing search to {uri}: {request:?}");

            let request_body =
                serde_json::to_string(&request).expect("Failed to serialize request");

            let http_request = Request::post(&uri)
                .header("Content-Type", "application/json")
                .body(request_body)
                .map_err(SearchError::HttpRequest)?;

            let mut response = self
                .http_client
                .send(http_request)
                .map_err(SearchError::HttpSend)?;

            let mut body = String::new();
            response
                .body_mut()
                .read_to_string(&mut body)
                .map_err(SearchError::ReadResponse)?;

            debug!("response: {}", body);

            if let Some(recorder) = &mut recorder {
                recorder
                    .record_response(&body)
                    .map_err(SearchError::RecordBody)?;
            }

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

    fn get_comments(&self, issue_key: String) -> BoxFuture<Result<Vec<Comment>, GetCommentError>> {
        async move {
            let mut start_at = 0i64;
            let max_results = i32::MAX;

            let mut comments = Vec::new();

            let mut recorder = match &self.record_path {
                Some(p) => Some(ApiRecorder::new(p.join(format!("get_comments/{issue_key}"))).map_err(GetCommentError::CreateRecorder)?),
                None => None,
            };

            loop {
                let uri = format!("{uri}/rest/api/3/issue/{issue_key}/comment?expand=renderedBody&startAt={start_at}&maxResults={max_results}", uri = self.uri);

                debug!("Retrieving comments at {uri}");

                let mut response = self
                    .http_client
                    .get_async(&uri)
                    .await
                    .map_err(GetCommentError::HttpSend)?;

                let mut body = String::new();
                response
                    .body_mut()
                    .read_to_string(&mut body)
                    .await
                    .map_err(GetCommentError::ReadResponse)?;

                debug!("response: {}", body);

                if let Some(recorder) = &mut recorder {
                    recorder.record_response(&body).map_err(GetCommentError::RecordBody)?;
                }

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
        }.boxed()
    }
}

pub struct ReplayJiraClient {
    replay_path: PathBuf,
}

impl ReplayJiraClient {
    pub fn new(replay_path: PathBuf) -> ReplayJiraClient {
        ReplayJiraClient { replay_path }
    }
}

impl JiraClient for ReplayJiraClient {
    fn execute_search(&self, _jql: String) -> Result<Vec<IssueBean>, SearchError> {
        // We ignore the JQL, as we're re-using whatever was recorded with
        let mut issues = Vec::new();

        let dir_iter = fs::read_dir(self.replay_path.join("execute_search"))
            .map_err(SearchError::ReadReplayDir)?;

        for item in dir_iter {
            let item = item.map_err(SearchError::ReadReplayDir)?;
            let body = fs::read(item.path()).map_err(SearchError::ReadReplayFile)?;
            let response: SearchResponse =
                serde_json::from_slice(&body).map_err(SearchError::SearchResponseParse)?;

            issues.extend(response.issues)
        }

        Ok(issues)
    }

    fn get_comments(&self, issue_key: String) -> BoxFuture<Result<Vec<Comment>, GetCommentError>> {
        async move {
            let mut comments = Vec::new();

            let dir_iter = fs::read_dir(self.replay_path.join(format!("get_comments/{issue_key}")))
                .map_err(GetCommentError::ReadReplayDir)?;

            for item in dir_iter {
                let item = item.map_err(GetCommentError::ReadReplayDir)?;
                let body = fs::read(item.path()).map_err(GetCommentError::ReadReplayFile)?;
                let response: GetCommentsResponse = serde_json::from_slice(&body)
                    .map_err(GetCommentError::GetCommentResponseParse)?;

                comments.extend(response.comments)
            }

            Ok(comments)
        }
        .boxed()
    }
}

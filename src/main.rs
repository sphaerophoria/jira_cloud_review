use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use futures::prelude::*;
use isahc::{
    auth::{Authentication, Credentials},
    prelude::*,
    Request,
};
use lol_html::{element, HtmlRewriter, Settings};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use futures::future::BoxFuture;

use std::{
    error::Error,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

mod jira_api;

use jira_api::{HttpJiraClient, JiraClient, ReplayJiraClient};

const WATER_CSS: &[u8] = include_bytes!("../res/water.css");
const INDEX_CSS: &[u8] = include_bytes!("../res/index.css");
const INDEX_JS: &[u8] = include_bytes!("../res/index.js");

#[derive(Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
struct AuthData {
    user: String,
    password: String,
}

#[derive(Debug)]
enum GetAuthError {
    NoConfigDir,
    AuthPathNotFile,
    AuthDirCreate(io::Error),
    AuthFileCreate(io::Error),
    SampleSerialize(serde_json::Error),
    AuthFileOpen(io::Error),
    Deserialize(serde_json::Error),
}

impl fmt::Display for GetAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GetAuthError::NoConfigDir => write!(f, "no config dir"),
            GetAuthError::AuthPathNotFile => write!(f, "auth config is not a file"),
            GetAuthError::AuthDirCreate(_) => write!(f, "could not create auth dir"),
            GetAuthError::AuthFileCreate(_) => write!(f, "failed to create auth file"),
            GetAuthError::SampleSerialize(_) => write!(f, "failed to serialize auth file"),
            GetAuthError::AuthFileOpen(_) => write!(f, "failed to open auth file"),
            GetAuthError::Deserialize(_) => write!(f, "failed to deserialize auth file"),
        }
    }
}

impl Error for GetAuthError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            GetAuthError::NoConfigDir => None,
            GetAuthError::AuthPathNotFile => None,
            GetAuthError::AuthDirCreate(e) => Some(e),
            GetAuthError::AuthFileCreate(e) => Some(e),
            GetAuthError::SampleSerialize(e) => Some(e),
            GetAuthError::AuthFileOpen(e) => Some(e),
            GetAuthError::Deserialize(e) => Some(e),
        }
    }
}

fn get_auth_path() -> Option<PathBuf> {
    let config_dir = match dirs::config_dir() {
        Some(v) => v,
        None => return None,
    };
    Some(
        config_dir
            .join("sphaerophroia")
            .join("jira_auth")
            .join("auth.json"),
    )
}

fn get_auth() -> Result<AuthData, GetAuthError> {
    let auth_path = get_auth_path().ok_or(GetAuthError::NoConfigDir)?;

    let auth_path_exists = match fs::metadata(&auth_path) {
        Ok(meta) => {
            if !meta.is_file() {
                return Err(GetAuthError::AuthPathNotFile);
            }
            true
        }
        Err(_) => false,
    };

    if !auth_path_exists {
        fs::create_dir_all(auth_path.parent().expect("Invalid auth path"))
            .map_err(GetAuthError::AuthDirCreate)?;

        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&auth_path)
            .map_err(GetAuthError::AuthFileCreate)?;

        serde_json::to_writer(&mut f, &AuthData::default())
            .map_err(GetAuthError::SampleSerialize)?;
    }

    let f = fs::File::open(auth_path).map_err(GetAuthError::AuthFileOpen)?;

    serde_json::from_reader(f).map_err(GetAuthError::Deserialize)
}

#[derive(Debug)]
enum ImageDownloadError {
    HttpRequest(isahc::http::Error),
    HttpSend(isahc::Error),
    CreateDir(io::Error),
    WriteFail(io::Error),
    ReadImg(io::Error),
    OpenOutput(io::Error),
    DecodePath(std::string::FromUtf8Error),
}

impl fmt::Display for ImageDownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageDownloadError::HttpRequest(_) => {
                write!(f, "failed to generate image download request")
            }
            ImageDownloadError::HttpSend(_) => write!(f, "failed to send image download request"),
            ImageDownloadError::CreateDir(_) => write!(f, "failed to create output directory"),
            ImageDownloadError::WriteFail(_) => write!(f, "failed to write output image"),
            ImageDownloadError::ReadImg(_) => write!(f, "failed to read image"),
            ImageDownloadError::OpenOutput(_) => write!(f, "failed to open output file"),
            ImageDownloadError::DecodePath(_) => write!(f, "failed to decode url"),
        }
    }
}

impl Error for ImageDownloadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ImageDownloadError::HttpRequest(e) => Some(e),
            ImageDownloadError::HttpSend(e) => Some(e),
            ImageDownloadError::CreateDir(e) => Some(e),
            ImageDownloadError::WriteFail(e) => Some(e),
            ImageDownloadError::ReadImg(e) => Some(e),
            ImageDownloadError::OpenOutput(e) => Some(e),
            ImageDownloadError::DecodePath(e) => Some(e),
        }
    }
}

async fn download_src(
    uri: &str,
    credentials: &Credentials,
    src: String,
    output: &Path,
) -> Result<(), ImageDownloadError> {
    // Only download the image if it's on jira's servers
    if !src.starts_with('/') {
        return Ok(());
    }

    // Strip leading /
    let mut src = &src[1..];

    // Strip trailing query params
    if let Some(pos) = src.find('?') {
        src = &src[..pos];
    }

    let decoded_src = urlencoding::decode(src).map_err(ImageDownloadError::DecodePath)?;
    let dst = output.join(decoded_src.as_ref());
    debug!("output: {}", dst.display());

    let img_uri = format!("{uri}/{src}");

    if dst.exists() {
        info!(
            "Skipping {img_uri}, output {dst} exists",
            dst = dst.display()
        );
        return Ok(());
    }

    info!("Downloading: {img_uri}");

    let mut img_response = Request::get(&img_uri)
        .credentials(credentials.clone())
        .authentication(Authentication::basic())
        .redirect_policy(isahc::config::RedirectPolicy::Follow)
        .body(())
        .map_err(ImageDownloadError::HttpRequest)?
        .send_async()
        .await
        .map_err(ImageDownloadError::HttpSend)?;

    debug!("{:?}", img_response);
    let mut img_data = Vec::new();

    img_response
        .body_mut()
        .read_to_end(&mut img_data)
        .await
        .map_err(ImageDownloadError::ReadImg)?;

    fs::create_dir_all(dst.parent().unwrap_or(output)).map_err(ImageDownloadError::CreateDir)?;

    let f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(dst)
        .map_err(ImageDownloadError::OpenOutput)?;

    let mut f = io::BufWriter::new(f);
    f.write_all(&img_data)
        .map_err(ImageDownloadError::WriteFail)?;

    Ok(())
}

struct DomWalkResult<'a> {
    healed_html: Vec<u8>,
    download_futures: Vec<BoxFuture<'a, Result<(), ImageDownloadError>>>,
}

fn download_and_heal_absolute_links<'a>(
    uri: &'a str,
    credentials: &'a Credentials,
    output_path: &'a Path,
    html: &str,
) -> Result<DomWalkResult<'a>, lol_html::errors::RewritingError> {
    let mut healed_html = Vec::new();
    let mut download_futures = Vec::new();

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![element!("[src]", |el| {
                let src = el
                    .get_attribute("src")
                    .expect("src attribute required by selector");

                if let Some(relative) = src.strip_prefix('/') {
                    el.set_attribute("src", relative)
                        .expect("src attribute could not be set");
                }

                download_futures
                    .push(download_src(uri, credentials, src.to_string(), output_path).boxed());

                Ok(())
            })],
            ..Settings::default()
        },
        |c: &[u8]| healed_html.extend_from_slice(c),
    );

    rewriter.write(html.as_bytes())?;

    drop(rewriter);

    let result = DomWalkResult {
        healed_html,
        download_futures,
    };

    Ok(result)
}

#[derive(Debug)]
enum ArgParseError {
    MissingUri,
    MissingUser,
    MissingOutput,
    MissingDate,
    InvalidDate(chrono::ParseError),
    InvalidArg(String),
}

impl fmt::Display for ArgParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgParseError::MissingUri => write!(f, "missing uri"),
            ArgParseError::MissingUser => write!(f, "missing user"),
            ArgParseError::MissingOutput => write!(f, "missing output"),
            ArgParseError::MissingDate => write!(f, "missing date"),
            ArgParseError::InvalidDate(_) => write!(f, "invalid date"),
            ArgParseError::InvalidArg(s) => write!(f, "invalid arg: {s}"),
        }
    }
}

impl Error for ArgParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ArgParseError::MissingUri => None,
            ArgParseError::MissingUser => None,
            ArgParseError::MissingOutput => None,
            ArgParseError::MissingDate => None,
            ArgParseError::InvalidDate(e) => Some(e),
            ArgParseError::InvalidArg(_) => None,
        }
    }
}

struct Args {
    uri: String,
    user: String,
    output: PathBuf,
    start_date: NaiveDate,
    record_path: Option<PathBuf>,
    replay_path: Option<PathBuf>,
}

impl Args {
    fn help() -> String {
        let process_name = std::env::current_exe().unwrap_or_else(|_| "jira_api_test".into());
        format!(
            "\
                 Usage:\n\
                 {process_name} [opts]\n\
                 \n\
                 Example:\n\
                 {process_name} --uri https://my-jira-instance.atlassian.net --user \"Display Name\" --output output_folder\n\
                 \n\
                 Required Options:\n\
                 --uri          Uri to jira instance\n\
                 --user         Display name to filter results with\n\
                 --output       Where to output the processed data\n\
                 --start-date   Initial date in form of YYYY-MM-DD\n\
                 \n\
                 Optional Options:\n\
                 --record-path  Where to store intermediate outputs, debugging feature\n\
                 --replay-path  If provided, no http requests will be made, and the output will be\n\
                                re-constructed from the recorded results of the recording\n\
                 ",
            process_name = process_name.display()
        )
    }

    fn from_iter<Iter, T>(mut iter: Iter) -> Result<Args, ArgParseError>
    where
        T: AsRef<str>,
        Iter: Iterator<Item = T>,
    {
        // Skip process name
        iter.next();

        let mut uri = None;
        let mut user = None;
        let mut output = None;
        let mut date = None;
        let mut record_path = None;
        let mut replay_path = None;
        while let Some(item) = iter.next() {
            let item = item.as_ref();
            match item {
                "--help" => {
                    eprintln!("{}", Args::help());
                    std::process::exit(1);
                }
                "--user" => user = iter.next(),
                "--uri" => uri = iter.next(),
                "--output" => output = iter.next(),
                "--record-path" => {
                    record_path = iter.next().map(|x| Path::new(x.as_ref()).to_owned());
                }
                "--replay-path" => {
                    replay_path = iter.next().map(|x| Path::new(x.as_ref()).to_owned());
                }
                "--start-date" => {
                    if let Some(val) = iter.next() {
                        date = Some(val.as_ref().parse().map_err(ArgParseError::InvalidDate)?);
                    }
                }
                s => return Err(ArgParseError::InvalidArg(s.to_string())),
            }
        }

        macro_rules! unwrap_val {
            ($val:expr, $err:expr) => {
                $val.map(|s| s.as_ref().to_string()).ok_or($err)?
            };
        }

        let uri = unwrap_val!(uri, ArgParseError::MissingUri);
        let user = unwrap_val!(user, ArgParseError::MissingUser);
        let output = unwrap_val!(output, ArgParseError::MissingOutput);
        let output = output.into();
        let start_date = date.ok_or(ArgParseError::MissingDate)?;

        Ok(Args {
            uri,
            user,
            output,
            start_date,
            record_path,
            replay_path,
        })
    }
}

enum MainError {
    ArgParseError(ArgParseError),
    Auth(GetAuthError),
    ConstructClient(jira_api::JiraClientCreationError),
    HtmlRewrite(lol_html::errors::RewritingError),
    Search(jira_api::SearchError),
    GetComments(jira_api::GetCommentError),
    WriteCss(io::Error),
    WriteJs(io::Error),
    OpenOutput(io::Error),
    OutputWriteFailed(io::Error),
    DownloadImageFailed(ImageDownloadError),
    DefaultAuth,
}

impl fmt::Debug for MainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut source: Option<&dyn Error> = match self {
            MainError::ArgParseError(e) => {
                writeln!(f, "failed to parse arguments")?;
                Some(e)
            }
            MainError::Auth(e) => {
                writeln!(f, "failed to get authorization")?;
                Some(e)
            }
            MainError::ConstructClient(e) => {
                writeln!(f, "failed to construct jira client")?;
                Some(e)
            }
            MainError::HtmlRewrite(e) => {
                writeln!(f, "failed to rewrite html with relative src links")?;
                Some(e)
            }
            MainError::Search(e) => {
                writeln!(f, "failed to execute search")?;
                Some(e)
            }
            MainError::GetComments(e) => {
                writeln!(f, "failed to get comments")?;
                Some(e)
            }
            MainError::WriteCss(e) => {
                writeln!(f, "failed to write css to output")?;
                Some(e)
            }
            MainError::WriteJs(e) => {
                writeln!(f, "failed to write js to output")?;
                Some(e)
            }
            MainError::OpenOutput(e) => {
                writeln!(f, "failed to open output")?;
                Some(e)
            }
            MainError::OutputWriteFailed(e) => {
                writeln!(f, "failed to write to output")?;
                Some(e)
            }
            MainError::DownloadImageFailed(e) => {
                writeln!(f, "failed to download image")?;
                Some(e)
            }
            MainError::DefaultAuth => {
                writeln!(f, "auth is not populated with user data")?;
                None
            }
        };

        while let Some(e) = source {
            writeln!(f, "caused by: {e}")?;
            source = e.source();
        }
        Ok(())
    }
}

async fn get_comments_for_issue(
    client: &dyn JiraClient,
    issue: jira_api::IssueBean,
) -> Result<(jira_api::IssueBean, Vec<jira_api::Comment>), MainError> {
    // Look through comments to see if any were written by the given user
    info!("Retrieving comments for {}", issue.key);

    // FIXME: Sort by date maybe?
    let comments = client
        .get_comments(issue.key.clone())
        .await
        .map_err(MainError::GetComments)?;

    Ok((issue, comments))
}

async fn async_main() -> Result<(), MainError> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::from_iter(std::env::args()).map_err(MainError::ArgParseError)?;

    let auth = get_auth().map_err(MainError::Auth)?;
    if auth == AuthData::default() {
        eprintln!(
            "Invalid auth file, please fill in data at {}",
            get_auth_path().expect("Invalid auth path").display()
        );
        return Err(MainError::DefaultAuth);
    }

    let credentials = Credentials::new(auth.user, auth.password);

    let client: Box<dyn JiraClient> = match &args.replay_path {
        Some(v) => Box::new(ReplayJiraClient::new(v.to_path_buf())),
        None => Box::new(
            HttpJiraClient::new(&args.uri, &credentials, args.record_path)
                .map_err(MainError::ConstructClient)?,
        ),
    };

    let date_str = format!(
        "{}-{}-{}",
        args.start_date.year(),
        args.start_date.month(),
        args.start_date.day()
    );
    let jql = format!("updated >= {date_str}");

    info!("Retrieving all issues that match fiilter \"{jql}\"");

    let issues = client.execute_search(jql).map_err(MainError::Search)?;

    info!("Found {} issues", issues.len());

    fs::create_dir_all(&args.output).map_err(MainError::OutputWriteFailed)?;

    fs::write(args.output.join("water.css"), WATER_CSS).map_err(MainError::WriteCss)?;

    fs::write(args.output.join("index.css"), INDEX_CSS).map_err(MainError::WriteCss)?;

    fs::write(args.output.join("index.js"), INDEX_JS).map_err(MainError::WriteJs)?;

    let output_index_html = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(args.output.join("index.html"))
        .map_err(MainError::OpenOutput)?;

    let mut output_index_html = io::BufWriter::new(output_index_html);

    writeln!(
        output_index_html,
        "<head>\n\
         \t<link rel=\"stylesheet\" href=\"water.css\">\n\
         \t<link rel=\"stylesheet\" href=\"index.css\">\n\
         \t<meta content=\"text/html;charset=utf-8\" http-equiv=\"Content-Type\">\n\
         \t<meta content=\"utf-8\" http-equiv=\"encoding\">\n\
         \t<script src=\"index.js\" defer></script>\n\
         </head>\n\
         <body>"
    )
    .map_err(MainError::OutputWriteFailed)?;

    let issue_comments = futures::future::join_all(
        issues
            .into_iter()
            .map(|issue| get_comments_for_issue(&*client, issue)),
    )
    .await;

    let mut download_futures = Vec::new();

    for issue_comment in issue_comments {
        let (issue, comments) = match issue_comment {
            Ok((i, c)) => (i, c),
            Err(e) => {
                // FIXME: map issue
                error!("Failed to fetch comments for issue ???: {e:?}");
                continue;
            }
        };

        let filtered_comments: Vec<_> = comments
            .into_iter()
            .map(|comment| {
                // FIXME: unwrap
                let comment_time = DateTime::<FixedOffset>::parse_from_str(
                    &comment.updated,
                    "%Y-%m-%dT%H:%M:%S%.3f%z",
                )
                .unwrap();
                (comment, comment_time)
            })
            .filter(|(comment, comment_time)| {
                comment.author.display_name == args.user
                    && comment_time.date_naive() >= args.start_date
            })
            .collect();

        if filtered_comments.is_empty() {
            continue;
        }

        // FIXME: do not show title if no comments pass filter
        writeln!(
            output_index_html,
            "\
               <h2 class=collapsible-header>\n\
               <a href={url}/browse/{issue_key}>{issue_key}</a>: {issue_summary}\n\
               </h2>",
            url = args.uri,
            issue_key = issue.key,
            issue_summary = issue.fields.summary
        )
        .map_err(MainError::OutputWriteFailed)?;

        writeln!(output_index_html, "<div class=collapsible-content>")
            .map_err(MainError::OutputWriteFailed)?;

        for (comment, comment_time) in filtered_comments {
            let comment_date = comment_time.date_naive();
            info!(
                "{} left comment on {} on {}, adding to output",
                args.user, issue.key, comment_date
            );

            writeln!(output_index_html, "<h3>{comment_time}</h3>")
                .map_err(MainError::OutputWriteFailed)?;

            let dom_walk_result = download_and_heal_absolute_links(
                &args.uri,
                &credentials,
                &args.output,
                &comment.rendered_body,
            )
            .map_err(MainError::HtmlRewrite)?;

            download_futures.extend(dom_walk_result.download_futures);

            output_index_html
                .write_all(&dom_walk_result.healed_html)
                .map_err(MainError::OutputWriteFailed)?;
        }

        writeln!(output_index_html, "</div>").map_err(MainError::OutputWriteFailed)?;
    }

    write!(output_index_html, "</body>").map_err(MainError::OutputWriteFailed)?;

    let download_futures = futures::future::join_all(download_futures).await;

    for res in download_futures {
        res.map_err(MainError::DownloadImageFailed)?;
    }

    Ok(())
}

fn main() -> Result<(), MainError> {
    futures::executor::block_on(async_main())
}

use std::{
    convert::Infallible,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use askama::Template;

use axum::{
    body::Bytes,
    extract::{multipart::MultipartError, Multipart},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
    routing::{get, post},
    Extension, Router, Server,
};

use futures_util::Stream;
use log::{debug, error, info};

use thiserror::Error;

use sqlx::SqlitePool;
use tokio::sync::watch::{self, Receiver, Sender};
use tower_http::services::ServeDir;

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate {
    name: String,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    error: SomeError,
}

#[derive(Template)]
#[template(path = "upload.html")]
struct UploadTemplate {
    src: String,
    format: String,
    size: usize,
    name: String,
}

async fn hello() -> HelloTemplate {
    debug!("baking the hello page");
    HelloTemplate {
        name: "buddy".to_owned(),
    }
}

struct UploadData {
    filename: String,
    data: Bytes,
    category: String,
}

#[derive(Error, Debug)]
enum SomeError {
    #[error("error parsing multipart formdata")]
    Multipart(#[from] MultipartError),
    #[error("{0} not found")]
    Option(&'static str),
    #[error("unable to save file")]
    SaveFile(#[from] tokio::io::Error),
    #[error("extension not found")]
    FileExt,
    #[error("failed to execute query")]
    Sqlx(#[from] sqlx::Error),
}

impl IntoResponse for SomeError {
    fn into_response(self) -> askama_axum::Response {
        error!("{:?}", &self);
        ErrorTemplate { error: self }.into_response()
    }
}

impl UploadData {
    async fn try_from_multipart(mut multipart: Multipart) -> Result<Self, SomeError> {
        let mut filename: Option<String> = None;
        let mut data: Option<Bytes> = None;
        let mut category: Option<String> = None;
        while let Some(field) = multipart.next_field().await? {
            let fieldname = field.name().ok_or(SomeError::Option("fieldname"))?;
            match fieldname {
                "image" => {
                    filename = Some(
                        field
                            .file_name()
                            .ok_or(SomeError::Option("filename"))?
                            .to_owned(),
                    );
                    data = Some(field.bytes().await?);
                }
                "category" => {
                    category = Some(field.text().await?);
                }
                _ => {}
            }
        }
        if let (Some(filename), Some(data), Some(category)) = (filename, data, category) {
            Ok(Self {
                filename,
                data,
                category,
            })
        } else {
            Err(SomeError::Option("fieldname, data, or category"))
        }
    }
}

async fn save_file(data: UploadData) -> Result<UploadTemplate, SomeError> {
    let Some(ext) = Path::new(&data.filename).extension() else {
        return Err(SomeError::FileExt);
    };
    let mut new_path = PathBuf::from("/tmp");
    new_path.push(data.category);
    new_path.push(uuid::Uuid::new_v4().to_string());
    new_path.set_extension(ext);
    tokio::fs::write(&new_path, &data.data).await?;
    Ok(UploadTemplate {
        src: new_path
            .to_string_lossy()
            .strip_prefix("/tmp")
            .unwrap()
            .to_string(),
        format: ext.to_string_lossy().to_string(),
        size: data.data.len(),
        name: data.filename,
    })
}

async fn upload(
    Extension(pool): Extension<SqlitePool>,
    multipart: Multipart,
) -> Result<UploadTemplate, SomeError> {
    let upload_data = UploadData::try_from_multipart(multipart).await?;
    let upload_data = save_file(upload_data).await?;
    let image_id = uuid::Uuid::new_v4().to_string();
    let size = upload_data.size as i64;
    sqlx::query!(
        r#"
            INSERT INTO images (image_id, src, format, size, name)
            VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        image_id,
        upload_data.src,
        upload_data.format,
        size,
        upload_data.name
    )
    .execute(&pool)
    .await?;
    Ok(upload_data)
}

async fn sse_handler(
    Extension(mut listeners): Extension<Receiver<String>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        while listeners.changed().await.is_ok() {
            let msg = listeners.borrow().clone();
            yield Ok(Event::default().data(msg))
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn send_notification(
    Extension(notifier): Extension<Arc<Sender<String>>>,
    message: String,
) -> &'static str {
    match notifier.send(message) {
        Ok(_) => "Hallejulah!",
        Err(_) => "Woopsie!",
    }
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    env_logger::init();
    info!("starting up!");
    let pool = SqlitePool::connect(&env::var("DATABASE_URL").expect("sqlite conn. url"))
        .await
        .expect("connect to db");
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("run sql migrations");
    let (notifier, listeners) = watch::channel(String::from("Hi!"));
    let app = Router::new()
        .route("/", get(hello).post(upload))
        .route("/sse", get(sse_handler).layer(Extension(listeners)))
        .route(
            "/notify",
            post(send_notification).layer(Extension(Arc::new(notifier))),
        )
        .layer(Extension(pool))
        .fallback_service(ServeDir::new("/tmp"));
    let addr = SocketAddr::from(([127, 0, 0, 1], 8001));
    Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

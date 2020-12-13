use actix_web::web;
use actix_web::HttpServer;
use actix_web::App;
use actix_files::Files;
// use actix_cors::Cors;

mod routes;
mod download_manager;

use routes::*;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if let Err(err_string) = download_manager::initialize() {
        println!("Failed to initialize download_manager because:\n{}", err_string);
        let error = std::io::ErrorKind::Other;
        return Err(error.into());
    }
    let config_res = download_manager::get_config();
    if let Err(err_string) = config_res {
        println!("Failed to initialize config because:\n{}", err_string);
        let error = std::io::ErrorKind::Other;
        return Err(error.into());
    }
    let config = config_res.unwrap();

    let local = tokio::task::LocalSet::new();
    let sys = actix_web::rt::System::run_in_tokio("server", &local);
    let _ = HttpServer::new(move || {
        App::new()
            .route("/download", web_post!(download))
            .route("/get", web_post!(get_progresses))
            .route("/videos", web_get!(list_source_videos))
            .service(Files::new("/img/", config.download_dir.clone()))
            .service(Files::new("/", config.frontend_dir.clone()).index_file("index.html"))
    })
        .bind("0.0.0.0:4000")?
        .run()
        .await?;
    sys.await?;
    Ok(())
}

#[macro_export]
macro_rules! web_get {
    ($token:tt) => {
        web::get().to($token)
    };
}
#[macro_export]
macro_rules! web_post {
    ($token:tt) => {
        web::post().to($token)
    };
}

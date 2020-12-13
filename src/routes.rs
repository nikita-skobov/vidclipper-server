use actix_web::HttpResponse;
use actix_web::web;
use progresslib2_server_extension::get_all_progresses_json;
use progresslib2_server_extension::GetProgressRequest;
use serde::Serialize;

use super::download_manager;
use super::download_manager::DownloadRequest;


pub async fn get_progresses(item: Option<web::Json<GetProgressRequest>>) -> HttpResponse {
    let json_request_option = item.map_or_else(|| None, |o| Some(o.0));
    get_all_progresses_json(json_request_option, &download_manager::PROGHOLDER)
}

pub async fn download(item: web::Json<DownloadRequest>) -> HttpResponse {
    let mut download_request = item.0;

    let using_name = match download_request.name {
        Some(ref name) => name.clone(),
        None => {
            let name = download_manager::random_download_name();
            download_request.name = Some(name.clone());
            name
        }
    };

    // if the above url does not exist, or if it is in an errored state
    // then we can start download
    match download_manager::start_download(
        download_request
    ) {
        Ok(_) => HttpResponse::Ok().body(using_name).into(),
        Err(e) => make_internal_error(format!("Failed to start download: {}", e)),
    }
}

#[derive(Debug, Default, Serialize)]
pub struct SourceVideo {
    pub url: String,
    pub video_data: Option<String>,
    pub thumbnail_data: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

pub async fn list_source_videos() -> HttpResponse {
    let downloaded_video_list = match download_manager::list_all_downloaded_videos() {
        Err(e) => return make_internal_error(format!("Failed to list videos: {}", e)),
        Ok(list) => list,
    };

    let mut out_vec = vec![];
    for (url, video_struct) in downloaded_video_list {
        let thumbnail_path = if let Some(location) = video_struct.thumbnail_location {
            let filename_string = location.file_name();
            if filename_string.is_none() {
                None
            } else {
                let filename_string = filename_string.unwrap().to_str();
                if filename_string.is_none() {
                    None
                } else {
                    let filename_string = filename_string.unwrap();
                    Some(format!("/img/{}", filename_string))
                }
            }
        } else {
            None
        };
        let video_path = if let Some(os_path_str) = video_struct.location.file_name() {
            if let Some(s) = os_path_str.to_str() {
                Some(format!("/img/{}", s))
            } else {
                None
            }
        } else {
            None
        };
        out_vec.push(SourceVideo {
            url,
            video_data: video_path,
            thumbnail_data: thumbnail_path,
            title: video_struct.title,
            description: video_struct.description,
        });
    }

    let json_string = match serde_json::to_string(&out_vec) {
        Err(e) => return make_internal_error(format!("Failed to serialize output: {}", e)),
        Ok(s) => s,
    };

    HttpResponse::Ok().body(json_string).into()
}

pub fn make_internal_error<S: AsRef<str>>(error_message: S) -> HttpResponse {
    HttpResponse::InternalServerError().body(
        error_message.as_ref().to_string()
    )
}

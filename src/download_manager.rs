use lazy_static::lazy_static;
use std::sync::Mutex;
use std::collections::HashMap;
use progresslib2::*;
use rand::prelude::*;
use rand::distributions::Alphanumeric;
use serde::{Deserialize, Serialize};
use std::process::ExitStatus;
use std::io::Error;
use tokio::process::Command;
use tokio::process::Child;
use tokio::process::ChildStdout;
use tokio::process::ChildStderr;
use tokio::io::Lines;
use tokio::fs;
use tokio::io::{BufReader, AsyncBufReadExt};
use std::{path::{PathBuf, Path}, process::Stdio, fmt::Display};

#[path = "./youtubedl_stage.rs"]
mod youtubedl_stage;
use youtubedl_stage::download_video;

#[path = "./cut_video_stage.rs"]
mod cut_video_stage;
use cut_video_stage::cut_video;

#[path = "./transcode_clip_stage.rs"]
mod transcode_clip_stage;
use transcode_clip_stage::transcode_clip;

#[path = "./data_store.rs"]
mod data_store;
use data_store::initialize_data;
use data_store::DownloadedVideos;
use data_store::DownloadedVideo;

pub const FAILED_TO_ACQUIRE_LOCK: &'static str = "Failed to acquire lock";
pub const DATA_STORE_PATH: &'static str = "vidclipper_data.json";

lazy_static! {
    pub static ref PROGHOLDER: Mutex<ProgressHolder<String>> = Mutex::new(
        ProgressHolder::<String>::default()
    );
    static ref DATAHOLDER: Mutex<DownloadedVideos> = Mutex::new(DownloadedVideos::default());
}

pub fn string_error(e: impl Display) -> String {
    format!("{}", e)
}

pub fn fmt_string_error<S: AsRef<str>>(s: S, e: impl Display) -> String {
    format!("{}: {}", s.as_ref(), e)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    pub name: Option<String>,
    pub start: Option<u32>,
    pub duration: Option<u32>,
    pub transcode_extension: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SplitRequest {
    pub start: Option<u32>,
    pub duration: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscodeRequest {
    pub transcode_extension: Option<String>,
    pub duration: Option<u32>,
}

// TODO: dont iter over all alphanumeric, we only
// want the lowercase ones...
pub fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .collect::<String>()
        .to_lowercase()
}

pub fn random_download_name() -> String {
    random_string(8)
}

/// provide an array/vec of string references where the first
/// element in the array is the executable name, and everything after
/// that is the arguments. note that options like: "-o ./src" should
/// be passed as two elements ie: [..., "-o", "./src", ...]
pub fn create_command<S: AsRef<str>>(exe_and_args: &[S]) -> Command {
    assert!(exe_and_args.len() >= 1);
    let mut cmd = Command::new(exe_and_args[0].as_ref());
    for i in 1..exe_and_args.len() {
        cmd.arg(exe_and_args[i].as_ref());
    }

    // it is assumed you want it piped,
    // but you can unset this yourself
    // after it is returned
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd
}

pub fn setup_child_and_reader(
    cmd: Command,
) -> Result<(Child, Lines<BufReader<ChildStdout>>, Lines<BufReader<ChildStderr>>), String> {
    let mut cmd = cmd;
    let mut child = cmd.spawn().map_err(
        |e| fmt_string_error("Failed to spawn child process", e))?;
    let stdout = child.stdout.take().map_or_else(
        || Err("Failed to get handle child process stdout"),
        |o| Ok(o))?;
    let stderr = child.stderr.take().map_or_else(
        || Err("Failed to get handle child process stderr"),
        |o| Ok(o))?;
    // create a reader from the handles we created
    let reader_stdout = BufReader::new(stdout).lines();
    let reader_stderr = BufReader::new(stderr).lines();
    Ok((child, reader_stdout, reader_stderr))
}

pub fn handle_child_exit(
    child_status: Result<ExitStatus, Error>
) -> Result<(), String> {
    let status = child_status.map_err(
        |e| fmt_string_error("child process encountered an error", e))?;

    match status.success() {
        true => Ok(()),
        false => {
            let error_code = status.code();
            if let None = error_code {
                return Err("child process failed to exit with a valid exit code".into());
            }
            let error_code = status.code().unwrap();
            let error_string = format!("child process exited with error code: {}", error_code);
            Err(error_string)
        }
    }
}

pub async fn find_file_paths_matching<S: AsRef<str>, P: AsRef<Path>>(
    matching: S,
    path: P,
) -> Result<Vec<PathBuf>, String> {
    let readdir = fs::read_dir(path).await;
    let mut readdir_entries = readdir.map_err(
        |e| fmt_string_error("Failed to read dir", e))?;

    let mut out_vec = vec![];
    loop {
        let direntry_opt = readdir_entries.next_entry().await.map_err(
            |e| fmt_string_error("Failed to iterate over dir", e))?;

        // I think if direntry_opt is None then this
        // means we read all files in this directory?
        let direntry = match direntry_opt {
            Some(d) => d,
            None => {
                if out_vec.len() == 0 {
                    return Err(format!("Failed to find anything matching {}", matching.as_ref()));
                }
                return Ok(out_vec);
            },
        };

        let file_name = direntry.file_name().to_str().map_or_else(
            || Err("Dir entry contains invalid characters"),
            |s| Ok(s.to_string()))?;

        if file_name.contains(matching.as_ref()) {
            out_vec.push(direntry.path());
            // // found the match
            // return Ok(direntry.path());
        }
    }
}

pub fn create_download_item(
    key: &String,
    download_request: DownloadRequest,
) -> ProgressItem {
    let url = download_request.url;
    let name = download_request.name;
    let name = match name {
        None => format!("clip.{}", &key),
        Some(ref s) => s.clone(),
    };

    // if the url already has been downloaded
    // we can skip the download stage
    let url_exists_at = match DATAHOLDER.lock() {
        Err(_) => None, // do nothing
        Ok(mut guard) => match guard.as_mut().get_mut(&url) {
            Some(path) => Some(path.location.to_owned()),
            None => None
        }
    };

    // temporarily removing transcode stage.
    // do we need it if the cut_video stage
    // is forcing h264 aac mp4 anyway?...
    // let transcode_stage = make_stage!(transcode_clip;
    //     key.clone(),
    //     TranscodeRequest {
    //         transcode_extension: download_request.transcode_extension,
    //         duration: download_request.duration,
    //     }
    // );

    let cut_stage = make_stage!(cut_video;
        key.clone(),
        name,
        SplitRequest {
            start: download_request.start,
            duration: download_request.duration,
        }
    );

    let mut progitem = ProgressItem::new();
    if let None = url_exists_at {
        let key_clone = key.clone();
        let url_clone = url.clone();
        let download_task = async move {
            let res = download_video(key_clone, url).await;
            let mut should_write_data_store = false;
            if let Ok(Some(progvars)) = &res {
                let original_download_path = progvars.clone_var::<PathBuf>("original_download_path");
                let original_thumbnail_path = progvars.clone_var::<PathBuf>("original_thumbnail_path");
                let ytdl_title = progvars.clone_var::<String>("ytdl_title");
                let ytdl_description = progvars.clone_var::<String>("ytdl_description");
                if original_download_path.is_some() {
                    should_write_data_store = true;
                    match DATAHOLDER.lock() {
                        Err(_) => {} // do nothing :shrug:
                        Ok(mut guard) => {
                            guard.as_mut().insert(url_clone, DownloadedVideo {
                                location: original_download_path.unwrap(),
                                thumbnail_location: original_thumbnail_path,
                                title: ytdl_title,
                                description: ytdl_description,
                            });
                        }
                    }
                }
            }

            if should_write_data_store {
                // write the data back to json
                // after this stage returns. no point for the
                // progress to wait for this to finish
                tokio::spawn(async move {
                    match DATAHOLDER.lock() {
                        Err(_) => {},
                        Ok(guard) => {
                            let _ = data_store::write_json_data(DATA_STORE_PATH, &guard);
                        },
                    }
                });
            }
            res
        };
        let download_stage = Stage::make("download_video", download_task);
        progitem.register_stage(download_stage);
    } else if let Some(original_download_path) = url_exists_at {
        // if the url does already exist, we want to
        // put a variable of the path where the other steps
        // can find this url
        progitem.insert_var("original_download_path", Box::new(original_download_path));
    }

    // the download_stage only happens if we havent downloaded
    // the video yet. in that case, it must happen
    // BEFORE the cut stage... obviously
    progitem.register_stage(cut_stage);
    // progitem.register_stage(transcode_stage);
    progitem
}

pub fn start_download(
    download_request: DownloadRequest
) -> Result<(), String>{
    let unique_key = random_string(16);
    let mut progitem = create_download_item(&unique_key, download_request);
    match PROGHOLDER.lock() {
        Err(_) => Err(FAILED_TO_ACQUIRE_LOCK.into()),
        Ok(mut guard) => {
            // here we start the progress item, and immediately hand it off
            // to the progholder. note that the start method also takes the progholder
            // but because it is currently under a lock, if the progress item tries
            // to use the progholder it will fail. Thats why internally, the progress item
            // uses try_lock to avoid blocking, and it has retry capabilities.
            progitem.start(unique_key.clone(), &PROGHOLDER);
            guard.progresses.insert(unique_key, progitem);
            Ok(())
        }
    }
}

pub fn list_all_downloaded_videos(
) -> Result<Vec<(String, DownloadedVideo)>, String> {
    let mut downloaded_map_clone = match DATAHOLDER.lock() {
        Err(_) => return Err(FAILED_TO_ACQUIRE_LOCK.into()),
        Ok(guard) => {
            // its probably faster to clone here
            // and iterate after the lock is done
            // than to iterate here?
            guard.clone()
        },
    };

    let mut out_vec = vec![];
    for (key, value) in downloaded_map_clone.as_mut().drain() {
        out_vec.push((key, value));
    }

    Ok(out_vec)
}

pub fn get_time_string_from_line<S: AsRef<str>>(line: S) -> Option<String> {
    let line = line.as_ref();
    let time_str = "out_time_us=";
    let time_len = time_str.len();
    let time_index = line.find(time_str)?;
    let time_index = time_index + time_len;
    // it is probably "00:00:00.00" format, so 11 chars
    let mut out_str = String::with_capacity(11);
    for c in line.chars().skip(time_index) {
        if c.is_whitespace() {
            break;
        }
        out_str.push(c);
    }
    Some(out_str)
}

pub fn get_millis_from_time_string<S: AsRef<str>>(time_string: S) -> Option<u32> {
    let time_string = time_string.as_ref();
    let micros = time_string.parse::<u64>().ok()?;
    Some(micros as u32 / 1000)
}

pub fn initialize() -> Result<(), String> {
    let mut data = initialize_data(DATA_STORE_PATH)?;
    let mut guard = DATAHOLDER.lock().map_err(string_error)?;

    let data_map = data.as_mut();
    let downloaded_videos_map = guard.as_mut();

    for (key, value) in data_map.drain() {
        downloaded_videos_map.insert(key, value);
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn find_file_path_by_match_works() {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let res = find_file_paths_matching("Cargo", ".").await;
            match res {
                Ok(pathvec) => {
                    assert!(pathvec.len() == 2);
                    let s1 = &pathvec[0];
                    let s2 = &pathvec[1];
                    assert!(
                        s1.to_str().unwrap().contains("Cargo.toml") ||
                        s2.to_str().unwrap().contains("Cargo.toml")
                    );
                },
                Err(_) => assert!(false)
            }
        });
    }
}

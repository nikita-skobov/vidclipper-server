use actix_web::web;
use actix_web::HttpServer;
use actix_web::HttpResponse;
use actix_web::App;
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
use tokio::io::Lines;
use tokio::io::{BufReader, AsyncBufReadExt};
use std::process::Stdio;

pub const FAILED_TO_ACQUIRE_LOCK: &'static str = "Failed to acquire lock";

lazy_static! {
    static ref PROGHOLDER: Mutex<ProgressHolder<String>> = Mutex::new(
        ProgressHolder::<String>::default()
    );
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    pub name: Option<String>,
    pub start: Option<u32>,
    pub duration: Option<u32>,
}

// TODO: dont iter over all alphanumeric, we only
// want the lowercase ones...
pub fn random_download_name() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .collect::<String>()
        .to_lowercase()
}

/// reads a given line from the output of youtube-dl
/// and parses it (very roughly and not perfectly)
/// and returns the value from 0 - 100, or None if it failed to parse
pub fn get_ytdl_progress(line: &str) -> Option<u8> {
    let mut ret_value = None;

    let percent_index = line.find('%');
    if let Some(percent_index) = percent_index {
        if line.contains("[download]") {
            let mut prev_index = percent_index;
            while line.get((prev_index - 1)..prev_index) != Some(" ") {
                prev_index -= 1;
                if prev_index == 0 {
                    break;
                }
            }
            let percent_string = line.get(prev_index..percent_index);
            if let Some(percent_string) = percent_string {
                if let Ok(percent_float) = percent_string.parse::<f64>() {
                    if ret_value.is_none() {
                        ret_value = Some(percent_float as u8);
                    }
                }
            }
        }
    }

    ret_value
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
    cmd: Command
) -> Result<(Child, Lines<BufReader<ChildStdout>>), String> {
    let mut cmd = cmd;
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let error_string = format!("Failed to spawn child process: {}", e);
            return Err(error_string);
        }
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let error_string = format!("Failed to get handle child process stdout");
            return Err(error_string);
        }
    };
    // create a reader from the stdout handle we created
    // pass that reader into the following future spawned on tokio
    let reader = BufReader::new(stdout).lines();
    Ok((child, reader))
}

pub fn handle_child_exit(
    child_status: Result<ExitStatus, Error>
) -> Result<(), String> {
    let status = match child_status {
        Ok(s) => s,
        Err(e) => {
            let error_string = format!("child process encountered an error: {}", e);
            return Err(error_string);
        }
    };

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

pub async fn download_video(
    url: String,
    output_name: Option<String>,
) -> TaskResult {
    // form the command via all of the args it needs
    // and do basic spawn error checking
    let mut exe_and_args = vec![
        "youtube-dl",
        "--newline",
        "--ignore-config",
        &url,
    ];
    let output_name = output_name.unwrap_or(String::from(""));
    if !output_name.is_empty() {
        exe_and_args.push("-o");
        exe_and_args.push(&output_name);
    }
    let cmd = create_command(&exe_and_args[..]);

    // create a reader from the stdout handle we created
    // pass that reader into the following future spawned on tokio
    let (child, mut reader) = setup_child_and_reader(cmd)?;
    tokio::spawn(async move {
        loop {
            let thing = reader.next_line().await;
            if let Err(_) = thing { break; }
            let thing = thing.unwrap();

            // break if we didnt get a line, ie: end of line
            if let None = thing {
                break;
            } else if let Some(ref line) = thing {
                let prog_opt = get_ytdl_progress(line);
                if let None = prog_opt { continue; }

                let progress = prog_opt.unwrap();
                use_me_from_progress_holder(&url, &PROGHOLDER, |me| {
                    println!("setting progress to {}", progress);
                    me.inc_progress_percent(progress as f64);
                });
            }
        }
    });

    // the above happens asynchronously, but here we await the child process
    // itself. as we await this child process, the above async future can run
    // whenever the reader finds a next line. But after here we actually return
    // our TaskResult that is read by the progresslib2
    let child_status = child.await;
    handle_child_exit(child_status)
}

pub fn create_download_item(
    download_request: DownloadRequest,
) -> ProgressItem {
    let url = download_request.url;
    let name = download_request.name;
    let name = match name {
        None => Some(random_download_name()),
        Some(ref s) => Some(s.clone()),
    };
    let download_stage = make_stage!(download_video;
        url,
        name,
    );
    let mut progitem = ProgressItem::new();
    progitem.register_stage(download_stage);
    progitem
}

pub fn start_download(
    download_request: DownloadRequest
) -> Result<(), String>{
    // since the url is required,
    // the url will be treated as the key.
    let url = download_request.url.clone();
    let mut progitem = create_download_item(download_request);
    match PROGHOLDER.lock() {
        Err(_) => Err(FAILED_TO_ACQUIRE_LOCK.into()),
        Ok(mut guard) => {
            // here we start the progress item, and immediately hand it off
            // to the progholder. note that the start method also takes the progholder
            // but because it is currently under a lock, if the progress item tries
            // to use the progholder it will fail. Thats why internally, the progress item
            // uses try_lock to avoid blocking, and it has retry capabilities.
            progitem.start(url.clone(), &PROGHOLDER);
            guard.progresses.insert(url, progitem);
            Ok(())
        }
    }
}

pub fn url_exists_in_progress<S: AsRef<str>>(url: S) -> Result<bool, &'static str> {
    match PROGHOLDER.lock() {
        Err(_) => Err(FAILED_TO_ACQUIRE_LOCK),
        Ok(guard) => Ok(guard.progresses.contains_key(url.as_ref())),
    }
}

pub fn url_exists_and_is_not_errored<S: AsRef<str>>(url: S) -> Result<bool, &'static str> {
    match PROGHOLDER.lock() {
        Err(_) => Err(FAILED_TO_ACQUIRE_LOCK),
        Ok(mut guard) => {
            match guard.progresses.get_mut(url.as_ref()) {
                None => Ok(false),
                Some(prog) => match prog.get_progress_error() {
                    None => Ok(true),
                    Some(_) => Ok(false),
                }
            }
        }
    }
}

pub fn get_progresses_info<S: AsRef<str>>(
    progress_keys: Vec<S>
) -> Result<HashMap<String, Vec<StageView>>, &'static str> {
    if progress_keys.is_empty() {
        return get_all_progresses_info();
    }

    match PROGHOLDER.lock() {
        Err(_) => Err(FAILED_TO_ACQUIRE_LOCK),
        Ok(mut guard) => {
            let mut hashmap = HashMap::<String, Vec<StageView>>::new();
            for key in progress_keys.iter() {
                match guard.progresses.get_mut(key.as_ref()) {
                    // if not found, just return a map with less
                    // entries than requested
                    None => {}
                    Some(progitem) => {
                        hashmap.insert(key.as_ref().to_string(), progitem.into());
                    }
                }
            }
            Ok(hashmap)
        }
    }
}

pub fn get_all_progresses_info() -> Result<HashMap<String, Vec<StageView>>, &'static str> {
    match PROGHOLDER.lock() {
        Err(_) => Err(FAILED_TO_ACQUIRE_LOCK),
        Ok(mut guard) => {
            let mut hashmap = HashMap::<String, Vec<StageView>>::new();
            for (key, progitem) in guard.progresses.iter_mut() {
                hashmap.insert(key.clone(), progitem.into());
            }
            Ok(hashmap)
        }
    }
}

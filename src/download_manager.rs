use actix_web::web;
use actix_web::HttpServer;
use actix_web::HttpResponse;
use actix_web::App;
use lazy_static::lazy_static;
use std::sync::Mutex;
use std::collections::HashMap;
use progresslib2::*;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::io::{BufReader, AsyncBufReadExt};
use std::process::Stdio;


lazy_static! {
    static ref PROGHOLDER: Mutex<ProgressHolder<String>> = Mutex::new(
        ProgressHolder::<String>::default()
    );
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

pub async fn download_video(
    url: String,
    output_name: Option<String>,
) -> TaskResult {
    // form the command via all of the args it needs
    // and do basic spawn error checking
    let mut cmd = Command::new("youtube-dl");
    cmd.arg("--newline");
    cmd.arg("--ignore-config");
    cmd.arg(&url);
    if let Some(output_name) = output_name {
        cmd.arg("-o");
        cmd.arg(output_name);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
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
    let mut reader = BufReader::new(stdout).lines();
    tokio::spawn(async move {
        loop {
            let thing = reader.next_line().await;
            if let Err(_) = thing { break; }
            let thing = thing.unwrap();

            if let Some(ref line) = thing {
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

pub fn create_download_item<S: AsRef<str>, N: AsRef<str>>(
    url: S,
    name: Option<N>,
) -> ProgressItem {
    let url = url.as_ref().to_string();
    let name = match name {
        None => None,
        Some(s) => Some(s.as_ref().to_string()),
    };
    let download_stage = make_stage!(download_video;
        url,
        name,
    );
    let mut progitem = ProgressItem::new();
    progitem.register_stage(download_stage);
    progitem
}

pub fn start_download<S: AsRef<str>>(
    url: S,
    name: Option<S>,
) -> Result<(), String>{
    let url = url.as_ref().to_string();
    let mut progitem = create_download_item(&url, name);
    match PROGHOLDER.lock() {
        Err(_) => Err("Failed to acquire lock".into()),
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
        Err(_) => Err("Failed to acquire lock"),
        Ok(guard) => Ok(guard.progresses.contains_key(url.as_ref())),
    }
}

pub fn url_exists_and_is_not_errored<S: AsRef<str>>(url: S) -> Result<bool, &'static str> {
    match PROGHOLDER.lock() {
        Err(_) => Err("Failed to acquire lock"),
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
        Err(_) => Err("Failed to acquire lock"),
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
        Err(_) => Err("Failed to acquire lock"),
        Ok(mut guard) => {
            let mut hashmap = HashMap::<String, Vec<StageView>>::new();
            for (key, progitem) in guard.progresses.iter_mut() {
                hashmap.insert(key.clone(), progitem.into());
            }
            Ok(hashmap)
        }
    }
}

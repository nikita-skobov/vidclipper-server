// use lazy_static::lazy_static;
// use std::sync::Mutex;
// use std::collections::HashMap;
use progresslib2::*;
// use rand::prelude::*;
// use rand::distributions::Alphanumeric;
// use serde::{Deserialize, Serialize};
// use std::process::ExitStatus;
// use std::io::Error;
// use tokio::process::Command;
// use tokio::process::Child;
// use tokio::process::ChildStdout;
// use tokio::process::ChildStderr;
// use tokio::io::Lines;
// use tokio::fs;
// use tokio::io::{BufReader, AsyncBufReadExt};
// use std::{path::{PathBuf, Path}, process::Stdio};

use super::create_command;
use super::setup_child_and_reader;
use super::handle_child_exit;
use super::find_file_path_by_match;
use super::PROGHOLDER;

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
    key: String,
    url: String,
    output_name: String,
) -> TaskResult {
    // form the command via all of the args it needs
    // and do basic spawn error checking
    let exe_and_args = vec![
        "youtube-dl",
        "--newline",
        "--ignore-config",
        &url,
        "-o",
        &output_name,
    ];
    println!("args: {:#?}", exe_and_args);
    let cmd = create_command(&exe_and_args[..]);

    // create a reader from the stdout handle we created
    // pass that reader into the following future spawned on tokio
    let (child, mut reader, mut stderr_reader) = setup_child_and_reader(cmd)?;
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
                use_me_from_progress_holder(&key, &PROGHOLDER, |me| {
                    println!("setting progress to {}", progress);
                    me.inc_progress_percent(progress as f64);
                });
            }
        }
    });
    // I think you need to process stderr on this one
    // otherwise it fails more often?....
    tokio::spawn(async move {
        loop {
            let thing = stderr_reader.next_line().await;
            if let Err(_) = thing { break; }
            let thing = thing.unwrap();
            if let None = thing {
                break;
            }
        }
    });

    // the above happens asynchronously, but here we await the child process
    // itself. as we await this child process, the above async future can run
    // whenever the reader finds a next line. But after here we actually return
    // our TaskResult that is read by the progresslib2
    let child_status = child.await;
    let res = handle_child_exit(child_status);
    let mut progvars = ProgressVars::default();
    if res.is_ok() {
        // say that we have downloaded this url
        // at the location. this gets put into
        // the progress vars so the next stage can read it
        // if necessary
        // TODO: dont assume current directory
        let output_path = find_file_path_by_match(&output_name, ".").await?;
        progvars.insert_var(
            "original_download_path",
            Box::new(output_path)
        );
    }
    res.map_or_else(|e| Err(e), |_| Ok(Some(progvars)))
}

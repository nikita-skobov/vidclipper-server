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
use std::{path::{PathBuf, Path}, process::Stdio};

#[path = "./youtubedl_stage.rs"]
mod youtubedl_stage;
use youtubedl_stage::download_video;


pub const FAILED_TO_ACQUIRE_LOCK: &'static str = "Failed to acquire lock";

lazy_static! {
    pub static ref PROGHOLDER: Mutex<ProgressHolder<String>> = Mutex::new(
        ProgressHolder::<String>::default()
    );
    static ref SOURCEHOLDER: Mutex<HashMap<String, PathBuf>> = Mutex::new(
        HashMap::new()
    );
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
    pub output_prefix: String,
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
    let stderr = match child.stderr.take() {
        Some(s) => s,
        None => {
            let error_string = format!("Failed to get handle child process stderr");
            return Err(error_string);
        }
    };
    // create a reader from the handles we created
    let reader_stdout = BufReader::new(stdout).lines();
    let reader_stderr = BufReader::new(stderr).lines();
    Ok((child, reader_stdout, reader_stderr))
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

pub async fn cut_video(
    key: String,
    split_request: SplitRequest,
) -> TaskResult {
    if split_request.duration.is_none() && split_request.start.is_none() {
        // no point in running ffmpeg just to copy the existing streams
        // to a new file.
        return Ok(None);
    }

    // previous step should have set the pathbuf of the file it
    // created, so we get that to be able to run ffmpeg
    // from the exact path of the input file
    let input_path: Option<PathBuf> = return_something_from_progress_holder(&key, &PROGHOLDER, |me| {
        me.clone_var::<PathBuf>("original_download_path")
    });

    let input_path = if input_path.is_none() {
        return Err("Failed to find input file".into());
    } else { input_path.unwrap() };

    let input_string = match input_path.to_str() {
        Some(s) => s.to_string(),
        None => {
            let error_string = format!("File path contains invalid characters: {:?}", input_path);
            return Err(error_string);
        }
    };
    let original_file_name = match input_path.file_name() {
        None => {
            let error_string = format!("File path contains invalid characters: {:?}", input_path);
            return Err(error_string);
        },
        Some(os_str) => match os_str.to_str() {
            Some(s) => s.to_string(),
            None => {
                let error_string = format!("File path contains invalid characters: {:?}", os_str);
                return Err(error_string);
            }
        },
    };
    let output_file_name = format!("{}{}", split_request.output_prefix, original_file_name);
    let cut_video_outpath = PathBuf::from(&output_file_name);

    let mut exe_and_args = vec![
        "ffmpeg".into(),
        "-loglevel".into(),
        "error".into(),
        "-hide_banner".into(),
        "-stats".into(),
        "-progress".into(),
        "pipe:1".into(),
    ];
    if let Some(ref start) = split_request.start {
        exe_and_args.push("-ss".into());
        exe_and_args.push(start.to_string());
    }
    if let Some(ref duration) = split_request.duration {
        exe_and_args.push("-t".into());
        exe_and_args.push(duration.to_string());
    }
    exe_and_args.push("-i".into());
    exe_and_args.push(input_string);
    exe_and_args.push("-acodec".into());
    exe_and_args.push("copy".into());
    exe_and_args.push("-vcodec".into());
    exe_and_args.push("copy".into());
    exe_and_args.push("-y".into());
    exe_and_args.push(output_file_name);
    println!("running with commands:\n{:#?}", exe_and_args);
    let cmd = create_command(&exe_and_args[..]);

    // create a reader from the stdout handle we created
    // pass that reader into the following future spawned on tokio
    let (child, mut reader, _) = setup_child_and_reader(cmd)?;

    let duration_millis = match split_request.duration {
        None => 1, // TODO: find the total duration of the input file via ffprobe
        Some(d) => d * 1000,
    };
    tokio::spawn(async move {
        loop {
            let thing = reader.next_line().await;
            if let Err(e) = thing {
                println!("there was error: {}", e);
                break;
            }
            let thing = thing.unwrap();

            // break if we didnt get a line, ie: end of line
            if let None = thing {
                break;
            } else if let Some(ref line) = thing {
                let time_string = get_time_string_from_line(&line);
                if time_string.is_none() {
                    continue;
                }
                let time_millis = get_millis_from_time_string(time_string.unwrap());
                if time_millis.is_none() {
                    continue;
                }
                let time_millis = time_millis.unwrap();
                let mut progress = time_millis as f64 / duration_millis as f64;
                if progress > 1.0 { progress = 1.0 };
                println!("time_millis: {}, duration_millis: {}, progress: {}", time_millis, duration_millis, progress);
                use_me_from_progress_holder(&key, &PROGHOLDER, |me| {
                    me.inc_progress_percent_normalized(progress);
                });
            }
        }
    });

    // the above happens asynchronously, but here we await the child process
    // itself. as we await this child process, the above async future can run
    // whenever the reader finds a next line. But after here we actually return
    // our TaskResult that is read by the progresslib2
    let child_status = child.await;

    handle_child_exit(child_status).map_or_else(
        |e| Err(e),
        |_| {
            let mut progvars = ProgressVars::default();
            progvars.insert_var("cut_video", Box::new(cut_video_outpath));
            Ok(Some(progvars))
        },
    )
}

pub async fn transcode_clip(
    key: String,
    transcode_request: TranscodeRequest,
) -> TaskResult {
    if transcode_request.transcode_extension.is_none() {
        // no point in running ffmpeg just to copy the existing streams
        // to a new file.
        return Ok(None);
    }
    let extension = transcode_request.transcode_extension.unwrap();

    let input_path: Option<PathBuf> = return_something_from_progress_holder(&key, &PROGHOLDER, |me| {
        me.clone_var::<PathBuf>("cut_video")
    });

    let input_path = if input_path.is_none() {
        // if there was no cut video, transcode
        // from the source video instead
        return_something_from_progress_holder(&key, &PROGHOLDER, |me| {
            me.clone_var::<PathBuf>("original_download_path")
        })
    } else { input_path };

    let mut input_path = if input_path.is_none() {
        return Err("Failed to find input file".into());
    } else { input_path.unwrap() };

    let input_string = match input_path.to_str() {
        Some(s) => s.to_string(),
        None => {
            let error_string = format!("File path contains invalid characters: {:?}", input_path);
            return Err(error_string);
        }
    };
    input_path.set_extension(extension);
    let output_file_name = match input_path.file_name() {
        None => {
            let error_string = format!("File path contains invalid characters: {:?}", input_path);
            return Err(error_string);
        },
        Some(os_str) => match os_str.to_str() {
            Some(s) => s.to_string(),
            None => {
                let error_string = format!("File path contains invalid characters: {:?}", os_str);
                return Err(error_string);
            }
        },
    };

    let mut exe_and_args = vec![
        "ffmpeg".into(),
        "-loglevel".into(),
        "error".into(),
        "-hide_banner".into(),
        "-stats".into(),
        "-progress".into(),
        "pipe:1".into(),
    ];
    exe_and_args.push("-i".into());
    exe_and_args.push(input_string);
    exe_and_args.push("-y".into());
    exe_and_args.push(output_file_name);
    println!("running with commands:\n{:#?}", exe_and_args);
    let cmd = create_command(&exe_and_args[..]);

    // create a reader from the stdout handle we created
    // pass that reader into the following future spawned on tokio
    let (child, mut reader, _) = setup_child_and_reader(cmd)?;

    let duration_millis = match transcode_request.duration {
        None => 1, // TODO: find the total duration of the input file via ffprobe
        Some(d) => d * 1000,
    };
    tokio::spawn(async move {
        loop {
            let thing = reader.next_line().await;
            if let Err(e) = thing {
                println!("there was error: {}", e);
                break;
            }
            let thing = thing.unwrap();

            // break if we didnt get a line, ie: end of line
            if let None = thing {
                break;
            } else if let Some(ref line) = thing {
                let time_string = get_time_string_from_line(&line);
                if time_string.is_none() {
                    continue;
                }
                let time_millis = get_millis_from_time_string(time_string.unwrap());
                if time_millis.is_none() {
                    continue;
                }
                let time_millis = time_millis.unwrap();
                let mut progress = time_millis as f64 / duration_millis as f64;
                if progress > 1.0 { progress = 1.0 };
                println!("time_millis: {}, duration_millis: {}, progress: {}", time_millis, duration_millis, progress);
                use_me_from_progress_holder(&key, &PROGHOLDER, |me| {
                    me.inc_progress_percent_normalized(progress);
                });
            }
        }
    });

    // the above happens asynchronously, but here we await the child process
    // itself. as we await this child process, the above async future can run
    // whenever the reader finds a next line. But after here we actually return
    // our TaskResult that is read by the progresslib2
    let child_status = child.await;

    handle_child_exit(child_status).map_or_else(|e| Err(e), |_| Ok(None))
}

pub async fn find_file_path_by_match<S: AsRef<str>, P: AsRef<Path>>(
    matching: S,
    path: P,
) -> Result<PathBuf, String> {
    let readdir = fs::read_dir(path).await;
    let mut readdir_entries = match readdir {
        Err(e) => {
            let error_string = format!("Failed to read dir: {}", e);
            return Err(error_string);
        }
        Ok(entries) => entries,
    };

    loop {
        match readdir_entries.next_entry().await {
            Err(e) => {
                let error_string = format!("Failed to iterate over dir: {}", e);
                return Err(error_string);
            }
            Ok(direntry_opt) => match direntry_opt {
                None => {
                    // I think this means we read
                    // all files in this directory?
                    let error_string = format!("Failed to find file matching {}", matching.as_ref());
                    return Err(error_string);
                }
                Some(direntry) => {
                    let file_name = match direntry.file_name().to_str() {
                        Some(s) => s.to_string(),
                        None => return Err("Dir entry contains invalid characters".into()),
                    };
                    if file_name.contains(matching.as_ref()) {
                        // found the match
                        return Ok(direntry.path());
                    }
                }
            }
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
        None => random_download_name(),
        Some(ref s) => s.clone(),
    };

    // if the url already has been downloaded
    // we can skip the download stage
    let url_already_downloaded = match SOURCEHOLDER.lock() {
        Err(_) => false, // do nothing
        Ok(guard) => guard.contains_key(&url),
    };

    let output_clip_prefix = format!("clipped.{}.", name.clone());
    let transcode_stage = make_stage!(transcode_clip;
        key.clone(),
        TranscodeRequest {
            transcode_extension: download_request.transcode_extension,
            duration: download_request.duration,
        }
    );

    let cut_stage = make_stage!(cut_video;
        key.clone(),
        SplitRequest {
            start: download_request.start,
            duration: download_request.duration,
            output_prefix: output_clip_prefix,
        }
    );

    let mut progitem = ProgressItem::new();
    if !url_already_downloaded {
        let key_clone = key.clone();
        let url_clone = url.clone();
        let download_task = async move {
            let res = download_video(key_clone, url, name).await;
            if let Ok(Some(progvars)) = &res {
                if let Some(path) = progvars.clone_var::<PathBuf>("original_download_path") {
                    match SOURCEHOLDER.lock() {
                        Err(_) => {} // do nothing :shrug:
                        Ok(mut guard) => { guard.insert(url_clone, path); },
                    }
                }
            }
            res
        };
        let download_stage = Stage::make("download_video", download_task);
        progitem.register_stage(download_stage);
    }
    // the download_stage only happens if we havent downloaded
    // the video yet. in that case, it must happen
    // BEFORE the cut stage... obviously
    progitem.register_stage(cut_stage);
    progitem.register_stage(transcode_stage);
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn find_file_path_by_match_works() {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let res = find_file_path_by_match("Cargo", ".").await;
            match res {
                Ok(s) => {
                    let s = s.to_str().unwrap();
                    assert!(s.contains("Cargo.toml"))
                },
                Err(_) => assert!(false)
            }
        });
    }
}

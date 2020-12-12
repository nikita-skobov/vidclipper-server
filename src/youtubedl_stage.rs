use progresslib2::*;

use super::create_command;
use super::setup_child_and_reader;
use super::handle_child_exit;
use super::find_file_paths_matching;
use super::PROGHOLDER;
use std::path::PathBuf;

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
) -> TaskResult {
    let key_clone = key.clone();
    // form the command via all of the args it needs
    // and do basic spawn error checking
    let output_format = format!("{}.%(ext)s", &key);
    let exe_and_args = vec![
        "youtube-dl",
        "--newline",
        "--ignore-config",
        "--write-info-json",
        "--write-thumbnail",
        &url,
        "-o",
        &output_format,
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
        let (
            output_path,
            mut info_json_path,
            mut thumbnail_path
        ) = get_downloaded_paths(&key_clone).await?;

        println!("got output path: {:?}", output_path);
        progvars.insert_var(
            "original_download_path",
            Box::new(output_path)
        );
        if let Some(info_path) = info_json_path.take() {
            println!("got info path: {:?}", info_path);
            // TODO: read json file (asynchronously!)
            // and add variables to progress item about the
            // title, description, etc.
        }
        if let Some(thumbnail_path) = thumbnail_path.take() {
            println!("got thumbnail path: {:?}", thumbnail_path);
            progvars.insert_var(
                "original_thumbnail_path",
                Box::new(thumbnail_path)
            );
        }
    }
    res.map_or_else(|e| Err(e), |_| Ok(Some(progvars)))
}

// TODO: add more?
pub const VALID_THUMBNAIL_EXTENSIONS: [&str; 9] = [
    "jpg", "jpeg", "png", "JPG", "JPEG", "PNG", "webp",
    "gif", "bmp"
];
pub const VALID_VIDEO_EXTENSIONS: [&str; 10] = [
    "mp4", "mkv", "ts", "webm", "avi", "mov", "qt", "vob",
    "3gp", "wmv"
];

pub async fn get_downloaded_paths<S: AsRef<str>>(
    matching: S
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>), String> {
    // TODO: dont assume current directory
    let output_paths = find_file_paths_matching(matching, ".").await?;
    get_downloaded_paths_from_vec(output_paths)
}

pub fn get_downloaded_paths_from_vec(
    output_paths: Vec<PathBuf>
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>), String> {
    let mut output_path = None;
    let mut info_json_path = None;
    let mut thumbnail_path = None;
    for path in output_paths {
        if let Some(os_ext) = path.extension() {
            // we know this one is the info_json path
            if os_ext == "json" && info_json_path.is_none() {
                info_json_path = Some(path);
            } else if VALID_THUMBNAIL_EXTENSIONS.iter().any(|e| *e == os_ext) && thumbnail_path.is_none() {
                thumbnail_path = Some(path);
            } else if VALID_VIDEO_EXTENSIONS.iter().any(|e| *e == os_ext) && output_path.is_none() {
                output_path = Some(path);
            }
        }
    }

    if let Some(path) = output_path.take() {
        return Ok((
            path,
            info_json_path,
            thumbnail_path
        ));
    }

    Err("Failed to find output path after download".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_paths_after_download_works() {
        let output_paths = vec![
            "vid.info.json".into(),
            "vid.mp4".into(),
            "vid.jpg".into()
        ];
        let (
            output_path,
            info_path,
            thumbnail_path
        ) = get_downloaded_paths_from_vec(output_paths).unwrap();

        assert!(output_path.to_str().unwrap().contains("vid.mp4"));
        assert!(info_path.unwrap().to_str().unwrap().contains("vid.info.json"));
        assert!(thumbnail_path.unwrap().to_str().unwrap().contains("vid.jpg"));
    }
}

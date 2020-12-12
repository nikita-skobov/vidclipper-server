use super::setup_child_and_reader;
use super::create_command;
use super::PathBuf;
use super::return_something_from_progress_holder;
use super::SplitRequest;
use super::TaskResult;
use super::get_millis_from_time_string;
use super::get_time_string_from_line;
use super::PROGHOLDER;
use super::use_me_from_progress_holder;
use super::handle_child_exit;
use super::ProgressVars;


pub async fn cut_video(
    key: String,
    output_file_name: String,
    split_request: SplitRequest,
) -> TaskResult {
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

    let input_extension = input_path.extension().map_or_else(
        || None, |o| o.to_str()
    ).map_or_else(|| "mkv", |o| o);

    // output should have same extension as input
    let mut cut_video_outpath = PathBuf::from(&output_file_name);
    cut_video_outpath.set_extension(input_extension);
    let output_file_name = match cut_video_outpath.to_str() {
        Some(o) => o.to_string(),
        None => {
            return Err(format!("Failed to create output clip"));
        }
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
    if let Some(ref start) = split_request.start {
        exe_and_args.push("-ss".into());
        exe_and_args.push(start.to_string());
    }
    if let Some(ref duration) = split_request.duration {
        exe_and_args.push("-t".into());
        exe_and_args.push(duration.to_string());
    }
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

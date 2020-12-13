use std::path::Path;
use std::path::PathBuf;
use std::{fmt::Display, collections::HashMap};
use serde::{Deserialize, Serialize};


#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct DownloadedVideo {
    pub location: PathBuf,
    pub thumbnail_location: Option<PathBuf>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct DownloadedVideos {
    #[serde(flatten)]
    videos: HashMap<String, DownloadedVideo>,
}
impl AsRef<HashMap<String, DownloadedVideo>> for DownloadedVideos {
    fn as_ref(&self) -> &HashMap<String, DownloadedVideo> {
        &self.videos
    }
}
impl AsMut<HashMap<String, DownloadedVideo>> for DownloadedVideos {
    fn as_mut(&mut self) -> &mut HashMap<String, DownloadedVideo> {
        &mut self.videos
    }
}

pub fn string_error(e: impl Display) -> String {
    format!("{}", e)
}

pub fn read_json_data<P: AsRef<Path>>(path: P) -> Result<DownloadedVideos, String> {
    let json_string = std::fs::read_to_string(path).map_err(string_error)?;
    json_string_to_data(json_string)
}

pub fn json_string_to_data<S: AsRef<str>>(json_string: S) -> Result<DownloadedVideos, String> {
    let vid_map: DownloadedVideos = serde_json::from_str(
        json_string.as_ref()
    ).map_err(string_error)?;

    Ok(vid_map)
}

pub fn data_to_json_string(data: &DownloadedVideos) -> Result<String, String> {
    serde_json::to_string(data).map_err(string_error)
}

pub fn write_json_data<P: AsRef<Path>>(path: P, data: &DownloadedVideos) -> Result<(), String> {
    let json_string = data_to_json_string(data)?;
    std::fs::write(path, json_string).map_err(string_error)
}

pub fn initialize_data<P: AsRef<Path>>(path: P) -> Result<DownloadedVideos, String> {
    if !path.as_ref().exists() {
        // create it as an empty json file if this
        // is the first time initializing
        std::fs::write(path.as_ref(), "{}").map_err(string_error)?;
    }

    read_json_data(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_load_json_data() {
        let json_string = r#"
        {
            "a": { "url": "xyz", "location": "./", "thumbnail_location": "./" },
            "b": { "url": "qqq", "location": "qqq.txt", "thumbnail_location": "./" }
        }
        "#;

        let data = json_string_to_data(json_string).unwrap();
        let data_map = data.as_ref();
        assert!(data_map.contains_key("a"));
        assert!(data_map.contains_key("b"));

        let a_vid = &data_map["a"];
        assert_eq!(a_vid.location.to_str().unwrap(), "./");
    }
}

# vidclipper-server

This is the backend to [vidclipper-frontend](https://github.com/nikita-skobov/vidclipper-frontend).

This is a server meant to run continuosly, and provide an api that can be used to start downloads, and clips. These downloads and clips are stored in a json file managed by this application.

This application is written in rust and uses actix for the webserver.

# Getting started

## 1.

This server depends on static files existing from vidclipper-frontend. Lets build those first:

```sh
git clone https://github.com/nikita-skobov/vidclipper-frontend
cd vidclipper-frontend
npm install
npm run build
```

This will create a `build/` directory. This directory is all that we need, so just copy that somewhere and then you can delete the rest of that repository if you wish.

## 2.

Now let's build this server. First clone and cd into this directory with:

```sh
git clone https://github.com/nikita-skobov/vidclipper-server
cd vidclipper-server
```

Then you will need to modify the `Cargo.toml` file and remove or comment out the last few lines. Comment out any lines that look like:

```
[patch."https://github.com/nikita-skobov/X"]
X = { path = "../X" }
```

Next, you are ready to build:

```sh
cargo build --release
```

Now before you run the server, you will need to create
and edit the config file:

```sh
touch vidclipper_config.json

# the contents should be something like:
# {
#     "download_dir": "videodata",
#     "frontend_dir": "./build/"
# }
```

Where make sure the `frontend_dir` points to the directory where the built frontend files reside (see step 1). `download_dir` can be any folder. If it does not exist, then vidclipper-server will make it for you on first run.

## 3.

You can run the server by:

```sh
./target/release/vidclipper-server
```

Which will listen on port 4000 by default. So once its running, you can visit: `http://localhost:4000/` in your browser. If you don't see a webpage that means it did not find the static files that need to exist at the config's `frontend_dir` field.

# Troubleshooting

If you run into this issue:

```
error: failed to resolve patches for `https://github.com/nikita-skobov/progresslib2-server-extension`

Caused by:
  failed to load source for dependency `progresslib2-server-extension`
```

This is because cargo is trying to use the patch defined in the `Cargo.toml`. Remember to comment this out. If you already commented it out, then delete `Cargo.lock` and the `target/` folder and try building again.

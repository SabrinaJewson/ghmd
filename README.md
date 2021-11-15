# ghmd: markdown previewer in the style of GitHub

```
USAGE:
    ghmd [OPTIONS] <input> --token <token>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -p, --port <port>      The port the server should bind to [default: 39131]
        --theme <theme>    The theme to generate the resulting page using [default: dark]  [possible values: dark,
                           light]
        --title <title>    The title of the page. Defaults to the filename
    -t, --token <token>    The authorization token to use. You can create a personal one at
                           <https://github.com/settings/tokens>

ARGS:
    <input>    The markdown file to render
```

`ghmd` will start up a webserver on `localhost` that renders the given file using GitHub's markdown
API. Changes to the file will automatically cause the rendered page to be refreshed.

## Installation

```sh
cargo install --git https://github.com/KaiJewson/ghmd
```

## See also

I wrote this project after I was dissatisfied with these projects I found online:

- [Another project named ghmd](https://github.com/gilliek/ghmd)
- [Grip](https://github.com/joeyespo/grip)
- [github-markdown-preview](https://github.com/dmarcotte/github-markdown-preview)

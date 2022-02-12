# ghmd: markdown previewer in the style of GitHub

```
USAGE:
    ghmd [OPTIONS] --token <TOKEN> <INPUT>

ARGS:
    <INPUT>    The markdown file to render

OPTIONS:
    -h, --help               Print help information
    -o, --output <OUTPUT>    The HTML file to generate. If this is specified, no server will be
                             started and instead a single static file will be produced
    -p, --port <PORT>        The port the server should bind to [default: 39131]
    -t, --token <TOKEN>      The authorization token to use. You can create a personal one at
                             <https://github.com/settings/tokens> [env: GITHUB_TOKEN=]
        --theme <THEME>      The theme to generate the resulting page using [default: dark]
                             [possible values: dark, light]
        --title <TITLE>      The title of the page. Defaults to the filename
```

`ghmd` will start up a webserver on `localhost` that renders the given file using GitHub's markdown
API. Changes to the file will automatically cause the rendered page to be refreshed.

## Installation

```sh
cargo install --git https://github.com/SabrinaJewson/ghmd
```

## See also

I wrote this project after I was dissatisfied with these projects I found online:

- [Another project named ghmd](https://github.com/gilliek/ghmd)
- [Grip](https://github.com/joeyespo/grip)
- [github-markdown-preview](https://github.com/dmarcotte/github-markdown-preview)

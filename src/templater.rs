use anyhow::Context as _;
use clap::ArgEnum;
use serde::Serialize;
use tera::Tera;

pub(crate) struct Templater {
    title: Box<str>,
    theme: Theme,
    template: Tera,
}

impl Templater {
    pub(crate) fn new(title: Box<str>, theme: Theme) -> Self {
        let mut template = Tera::default();
        template.autoescape_on(Vec::new());
        template
            .add_raw_template("html", include_str!("template.html"))
            .unwrap();

        Self {
            title,
            theme,
            template,
        }
    }

    pub(crate) async fn generate(&self, html: &str, liveness: Liveness) -> anyhow::Result<String> {
        #[derive(Serialize)]
        struct HtmlTemplateOpts<'a> {
            title: &'a str,
            content: &'a str,
            theme: &'a str,
            javascript: &'a str,
        }
        self.template
            .render(
                "html",
                &tera::Context::from_serialize(HtmlTemplateOpts {
                    title: &self.title,
                    content: html,
                    theme: self.theme.as_str(),
                    javascript: match liveness {
                        Liveness::Static => include_str!("template.js"),
                        Liveness::Live => concat!(
                            include_str!("template_live.js"),
                            include_str!("template.js")
                        ),
                    },
                })
                .unwrap(),
            )
            .context("failed to render template")
    }
}

pub(crate) enum Liveness {
    Static,
    Live,
}

#[derive(Clone, Copy, ArgEnum)]
pub(crate) enum Theme {
    Dark,
    Light,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::Dark
    }
}

use owo_colors::{OwoColorize, Style as OwoStyle};

/// Applies colour and style to terminal text.
#[derive(Debug)]
pub(crate) struct Painter {
    use_colour: bool,
}

impl Painter {
    /// Creates a painter with explicit colour control.
    pub(crate) fn new(use_colour: bool) -> Self {
        Self { use_colour }
    }

    pub(crate) fn heading<T: AsRef<str>>(&self, text: T) -> String {
        self.paint(text.as_ref(), OwoStyle::new().bold().cyan())
    }

    pub(crate) fn success<T: AsRef<str>>(&self, text: T) -> String {
        self.paint(text.as_ref(), OwoStyle::new().bold().green())
    }

    pub(crate) fn warning<T: AsRef<str>>(&self, text: T) -> String {
        self.paint(text.as_ref(), OwoStyle::new().bold().yellow())
    }

    pub(crate) fn muted<T: AsRef<str>>(&self, text: T) -> String {
        self.paint(text.as_ref(), OwoStyle::new().dimmed())
    }

    pub(crate) fn value<T: AsRef<str>>(&self, text: T) -> String {
        self.paint(text.as_ref(), OwoStyle::new().bold())
    }

    fn paint(&self, text: &str, style: OwoStyle) -> String {
        if self.use_colour {
            format!("{}", text.style(style))
        } else {
            text.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn apply(painter: &Painter, style: &str, text: &str) -> String {
        match style {
            "heading" => painter.heading(text),
            "success" => painter.success(text),
            "warning" => painter.warning(text),
            "muted" => painter.muted(text),
            "value" => painter.value(text),
            other => panic!("unknown style: {other}"),
        }
    }

    #[rstest]
    #[case::heading("heading", "hello")]
    #[case::success("success", "ok")]
    #[case::warning("warning", "warn")]
    #[case::muted("muted", "dim")]
    #[case::value("value", "bold")]
    fn plain_returns_unstyled_text(#[case] style: &str, #[case] input: &str) {
        let painter = Painter::new(false);
        assert_eq!(input, apply(&painter, style, input));
    }

    #[rstest]
    #[case::heading("heading", "hello")]
    #[case::success("success", "ok")]
    #[case::warning("warning", "warn")]
    #[case::muted("muted", "dim")]
    #[case::value("value", "bold")]
    fn coloured_returns_styled_text(#[case] style: &str, #[case] input: &str) {
        let painter = Painter::new(true);
        let styled = apply(&painter, style, input);
        assert_ne!(styled, input);
        assert!(styled.contains(input));
    }
}

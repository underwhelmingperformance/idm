use std::fmt::{self, Display, Formatter};

use tabled::{builder::Builder, settings::Style as TableStyle};

use super::painter::Painter;

/// A structured table that renders via `Display`.
#[derive(Debug)]
pub(crate) struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// Creates a table with column headers and data rows.
    pub(crate) fn grid(
        headers: impl IntoIterator<Item = impl Into<String>>,
        rows: Vec<Vec<String>>,
    ) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows,
        }
    }

    /// Creates a two-column field/value table with muted field names.
    pub(crate) fn key_value(painter: &Painter, rows: Vec<(&str, String)>) -> Self {
        let records = rows
            .into_iter()
            .map(|(field, value)| vec![painter.muted(field), value])
            .collect();
        Self::grid(["field", "value"], records)
    }
}

impl Display for Table {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut builder = Builder::default();
        builder.push_record(&self.headers);
        for row in &self.rows {
            builder.push_record(row);
        }
        let mut table = builder.build();
        table.with(TableStyle::rounded());
        write!(f, "{table}")
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    #[test]
    fn grid_table_renders_with_headers_and_rows() {
        let table = Table::grid(
            ["name", "value"],
            vec![
                vec!["alpha".into(), "1".into()],
                vec!["beta".into(), "2".into()],
            ],
        );
        assert_snapshot!("grid_table", table.to_string());
    }

    #[test]
    fn key_value_table_renders_field_value_pairs() {
        let painter = Painter::new(false);
        let table = Table::key_value(
            &painter,
            vec![("host", "example.com".into()), ("port", "443".into())],
        );
        assert_snapshot!("key_value_table", table.to_string());
    }
}

use std::fmt::Display;

use serde::Serialize;

/// A single key/value row within a diagnostics section.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(crate) struct DiagnosticRow {
    label: String,
    value: String,
}

impl DiagnosticRow {
    /// Creates a diagnostics row.
    pub(crate) fn new(label: impl Into<String>, value: impl Display) -> Self {
        Self {
            label: label.into(),
            value: value.to_string(),
        }
    }

    /// Returns the row label.
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    /// Returns the row value.
    pub(crate) fn value(&self) -> &str {
        &self.value
    }
}

/// A diagnostics block emitted by a component.
pub(crate) trait DiagnosticsSection {
    /// Stable section identifier for de-duplication and machine parsing.
    fn section_id(&self) -> &'static str;

    /// Human-readable section heading.
    fn section_name(&self) -> &'static str;

    /// Rows to render for this section.
    fn rows(&self) -> Vec<DiagnosticRow>;
}

/// A container that exposes one or more diagnostics sections.
pub(crate) trait HasDiagnostics {
    /// Returns each diagnostics section from this container.
    fn diagnostics(&self) -> Vec<&dyn DiagnosticsSection>;
}

/// Immutable diagnostics snapshot stored in session metadata.
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize)]
pub(crate) struct ConnectionDiagnostics {
    sections: Vec<DiagnosticSectionSnapshot>,
}

impl ConnectionDiagnostics {
    /// Returns all captured diagnostics sections.
    pub(crate) fn sections(&self) -> &[DiagnosticSectionSnapshot] {
        &self.sections
    }

    /// Returns whether any diagnostics sections are present.
    pub(crate) fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

/// Captured section data detached from producer lifetimes.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(crate) struct DiagnosticSectionSnapshot {
    id: String,
    name: String,
    rows: Vec<DiagnosticRow>,
}

impl DiagnosticSectionSnapshot {
    fn from_section(section: &dyn DiagnosticsSection) -> Self {
        Self {
            id: section.section_id().to_string(),
            name: section.section_name().to_string(),
            rows: section.rows(),
        }
    }

    /// Returns the stable section identifier.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// Returns the section heading.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Returns section rows.
    pub(crate) fn rows(&self) -> &[DiagnosticRow] {
        &self.rows
    }
}

/// Builder used to aggregate diagnostics sections from loosely-coupled sources.
#[derive(Debug, Default)]
pub(crate) struct ConnectionDiagnosticsBuilder {
    sections: Vec<DiagnosticSectionSnapshot>,
}

impl ConnectionDiagnosticsBuilder {
    /// Creates an empty diagnostics builder.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a diagnostics section by its section id.
    pub(crate) fn push_section(&mut self, section: &dyn DiagnosticsSection) {
        let snapshot = DiagnosticSectionSnapshot::from_section(section);
        if let Some(existing_index) = self
            .sections
            .iter()
            .position(|existing| existing.id() == snapshot.id())
        {
            self.sections[existing_index] = snapshot;
            return;
        }

        self.sections.push(snapshot);
    }

    /// Adds every diagnostics section from a container.
    pub(crate) fn extend<T: HasDiagnostics>(&mut self, diagnostics: &T) {
        for section in diagnostics.diagnostics() {
            self.push_section(section);
        }
    }

    /// Finalises the immutable diagnostics snapshot.
    pub(crate) fn finish(self) -> ConnectionDiagnostics {
        ConnectionDiagnostics {
            sections: self.sections,
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[derive(Debug)]
    struct SectionA {
        value: &'static str,
    }

    impl DiagnosticsSection for SectionA {
        fn section_id(&self) -> &'static str {
            "section_a"
        }

        fn section_name(&self) -> &'static str {
            "Section A"
        }

        fn rows(&self) -> Vec<DiagnosticRow> {
            vec![DiagnosticRow::new("value", self.value)]
        }
    }

    #[derive(Debug)]
    struct SectionB;

    impl DiagnosticsSection for SectionB {
        fn section_id(&self) -> &'static str {
            "section_b"
        }

        fn section_name(&self) -> &'static str {
            "Section B"
        }

        fn rows(&self) -> Vec<DiagnosticRow> {
            vec![DiagnosticRow::new("enabled", "yes")]
        }
    }

    #[test]
    fn builder_replaces_duplicate_section_ids() {
        let mut builder = ConnectionDiagnosticsBuilder::new();
        builder.push_section(&SectionA { value: "old" });
        builder.push_section(&SectionB);
        builder.push_section(&SectionA { value: "new" });

        let diagnostics = builder.finish();
        let sections = diagnostics.sections();
        assert_eq!(2, sections.len());
        assert_eq!("section_a", sections[0].id());
        assert_eq!("new", sections[0].rows()[0].value());
        assert_eq!("section_b", sections[1].id());
    }
}

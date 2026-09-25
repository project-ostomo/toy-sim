#[derive(Clone, Debug, Default, PartialEq)]
pub enum QueryState<T> {
    #[default]
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> QueryState<T> {
    pub fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn as_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn completed(&self) -> bool {
        !matches!(self, Self::Loading)
    }

    pub fn apply(&mut self, changed: bool, result: Option<Result<T, String>>) {
        if changed {
            *self = Self::Loading;
        }
        if let Some(result) = result {
            *self = match result {
                Ok(value) => Self::Ready(value),
                Err(error) => Self::Failed(error),
            };
        }
    }
}

/// Returns true when the user requests another attempt.
pub fn render_query<T>(
    ui: &mut osg_ui::egui::Ui,
    state: &QueryState<T>,
    draw: impl FnOnce(&mut osg_ui::egui::Ui, &T),
) -> bool {
    match state {
        QueryState::Loading => {
            ui.spinner();
            ui.label("Loading…");
            false
        }
        QueryState::Ready(value) => {
            draw(ui, value);
            false
        }
        QueryState::Failed(error) => {
            ui.colored_label(osg_ui::egui::Color32::LIGHT_RED, error);
            ui.button("Retry").clicked()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_retains_value_but_changed_queries_and_failures_remove_it() {
        let mut state = QueryState::Ready("private balance");
        state.apply(false, None);
        assert_eq!(state.as_ref(), Some(&"private balance"));
        state.apply(false, Some(Err("Permission denied".into())));
        assert_eq!(state, QueryState::Failed("Permission denied".into()));
        assert!(state.as_ref().is_none());
        state.apply(false, Some(Ok("new balance")));
        state.apply(true, None);
        assert_eq!(state, QueryState::Loading);
    }
}

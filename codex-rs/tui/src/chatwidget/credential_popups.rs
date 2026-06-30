use super::*;
use crate::bottom_pane::sensitive_prompt_view::SensitivePromptView;
use crate::model_runtime::CredentialStatus;
use crate::model_runtime::OnboardingProvider;

impl ChatWidget {
    pub(crate) fn open_onboarding_provider_popup(&mut self, providers: Vec<OnboardingProvider>) {
        let items = providers
            .into_iter()
            .map(|provider| {
                let selected = provider.clone();
                let description = match provider.credential.status {
                    CredentialStatus::Missing => "API key required",
                    CredentialStatus::EnvironmentOverride => "Environment API key ready",
                    CredentialStatus::Verified => "Verified API key ready",
                    CredentialStatus::Unverified => "Saved API key ready",
                };
                SelectionItem {
                    name: provider.display_name,
                    description: Some(description.to_string()),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::SelectOnboardingProvider(selected.clone()));
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Select Service Provider".to_string()),
            subtitle: Some("Choose a provider before entering its API key and models.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            allow_cancel: false,
            ..Default::default()
        });
    }

    pub(crate) fn open_onboarding_credential_prompt(&mut self, provider: OnboardingProvider) {
        let provider_for_submit = provider.clone();
        let app_event_tx = self.app_event_tx.clone();
        let view = SensitivePromptView::new(
            format!("Enter {} API key", provider.display_name),
            "Paste an API key and press Enter".to_string(),
            Some(provider.credential.environment_variable.clone()),
            Box::new(move |value| {
                app_event_tx.send(AppEvent::StoreOnboardingCredential {
                    provider: provider_for_submit.clone(),
                    value,
                });
            }),
        )
        .with_cancel({
            let app_event_tx = self.app_event_tx.clone();
            Box::new(move || {
                app_event_tx.send(AppEvent::BeginModelRuntimeOnboarding);
            })
        });
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn open_credentials_popup_with_entries(&mut self, entries: Vec<CredentialEntry>) {
        if entries.is_empty() {
            self.add_info_message(
                "No model provider credentials are configured.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let items = entries
            .into_iter()
            .map(|entry| {
                let entry_for_action = entry.clone();
                let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenCredentialActions(entry_for_action.clone()));
                })];
                SelectionItem {
                    name: entry.display_name,
                    description: Some(credential_status_label(&entry.status).to_string()),
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Model Provider Credentials".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_credential_actions(&mut self, entry: CredentialEntry) {
        let items = match entry.status {
            CredentialStatus::EnvironmentOverride => {
                vec![environment_instructions_item(&entry)]
            }
            CredentialStatus::Verified | CredentialStatus::Unverified => vec![
                credential_prompt_item("Replace credential", &entry),
                revalidate_item(&entry),
                delete_item(&entry),
            ],
            CredentialStatus::Missing => {
                vec![credential_prompt_item("Enter credential", &entry)]
            }
        };

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(entry.display_name),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_credential_prompt(&mut self, entry: CredentialEntry) {
        self.show_credential_prompt(
            entry, /*continuation*/ None, /*cancel_selection*/ false,
        );
    }

    pub(crate) fn open_model_credential_prompt(&mut self) {
        let Some((entry, selection)) = self.pending_model_selection_for_credential.take() else {
            return;
        };
        self.show_credential_prompt(entry, Some(selection), /*cancel_selection*/ true);
    }

    pub(crate) fn open_submission_credential_prompt(
        &mut self,
        entry: CredentialEntry,
        model: String,
    ) {
        let app_event_tx = self.app_event_tx.clone();
        let entry_for_submit = entry.clone();
        let view = SensitivePromptView::new(
            "Enter credential".to_string(),
            "Paste a credential and press Enter".to_string(),
            Some(entry.display_name),
            Box::new(move |value| {
                app_event_tx.send(AppEvent::StoreCredentialForSubmission {
                    entry: entry_for_submit.clone(),
                    value,
                    model: model.clone(),
                });
            }),
        )
        .with_cancel({
            let app_event_tx = self.app_event_tx.clone();
            Box::new(move || {
                app_event_tx.send(AppEvent::CancelModelReadySubmission);
            })
        });
        self.bottom_pane.show_view(Box::new(view));
    }

    fn show_credential_prompt(
        &mut self,
        entry: CredentialEntry,
        continuation: Option<PendingModelSelection>,
        cancel_selection: bool,
    ) {
        let app_event_tx = self.app_event_tx.clone();
        let entry_for_submit = entry.clone();
        let mut view = SensitivePromptView::new(
            "Enter credential".to_string(),
            "Paste a credential and press Enter".to_string(),
            Some(entry.display_name),
            Box::new(move |value| {
                app_event_tx.send(AppEvent::StoreCredential {
                    entry: entry_for_submit.clone(),
                    value,
                    continuation: continuation.clone(),
                });
            }),
        );
        if cancel_selection {
            let app_event_tx = self.app_event_tx.clone();
            view = view.with_cancel(Box::new(move || {
                app_event_tx.send(AppEvent::CancelModelSelection);
            }));
        }
        self.bottom_pane.show_view(Box::new(view));
    }
}

fn credential_status_label(status: &CredentialStatus) -> &'static str {
    match status {
        CredentialStatus::EnvironmentOverride => "Environment override",
        CredentialStatus::Verified => "Verified",
        CredentialStatus::Unverified => "Unverified",
        CredentialStatus::Missing => "Missing",
    }
}

fn environment_instructions_item(entry: &CredentialEntry) -> SelectionItem {
    SelectionItem {
        name: "Environment override".to_string(),
        description: Some(format!(
            "Change {} in the environment that launches Codex.",
            entry.environment_variable
        )),
        is_disabled: true,
        ..Default::default()
    }
}

fn credential_prompt_item(name: &str, entry: &CredentialEntry) -> SelectionItem {
    let entry = entry.clone();
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenCredentialPrompt(entry.clone()));
    })];
    SelectionItem {
        name: name.to_string(),
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn revalidate_item(entry: &CredentialEntry) -> SelectionItem {
    let entry = entry.clone();
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::RevalidateCredential(entry.clone()));
    })];
    SelectionItem {
        name: "Revalidate credential".to_string(),
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn delete_item(entry: &CredentialEntry) -> SelectionItem {
    let entry = entry.clone();
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::DeleteCredential(entry.clone()));
    })];
    SelectionItem {
        name: "Delete credential".to_string(),
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

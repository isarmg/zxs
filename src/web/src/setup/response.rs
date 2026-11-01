//! The three bootstrap mutations have distinct, current success contracts.
use data_serialization::Deserialize;
use json_codec::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Action {
    #[default]
    Wizard,
    Preset,
    Complete,
}

#[derive(Debug, error_definition::Error)]
pub(super) enum ResponseError {
    #[error("invalid setup response: {0}")]
    Decode(#[source] json_codec::Error),
    #[error("setup response does not confirm initialization")]
    NotInitialized,
    #[error("setup response does not confirm completion")]
    NotCompleted,
    #[error("setup response has contradictory recovery and restart states")]
    RecoveryState,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Initialized {
    initialized: bool,
    restart_required: bool,
    recovering: bool,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Completed {
    initialized: bool,
    completed: bool,
    restart_required: bool,
    recovering: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Success {
    Recovering,
    RestartRequired,
}

pub(super) fn validate(action: Action, value: Value) -> Result<Success, ResponseError> {
    let Initialized {
        initialized,
        restart_required,
        recovering,
    } = match action {
        Action::Wizard | Action::Preset => {
            json_codec::from_value(value).map_err(ResponseError::Decode)?
        }
        Action::Complete => {
            let Completed {
                initialized,
                completed,
                restart_required,
                recovering,
            } = json_codec::from_value(value).map_err(ResponseError::Decode)?;
            if !completed {
                return Err(ResponseError::NotCompleted);
            }
            Initialized {
                initialized,
                restart_required,
                recovering,
            }
        }
    };
    if !initialized {
        return Err(ResponseError::NotInitialized);
    }
    match (restart_required, recovering) {
        (true, false) => Ok(Success::RestartRequired),
        (false, true) => Ok(Success::Recovering),
        _ => Err(ResponseError::RecoveryState),
    }
}

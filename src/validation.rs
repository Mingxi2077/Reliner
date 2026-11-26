use std::fmt;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::topology::Topology;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InputLocation {
    /// JSON Pointer into the submitted document; an empty string identifies the document.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputError {
    pub code: String,
    pub message: String,
    pub location: Box<InputLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

impl InputError {
    pub(crate) fn new(message: String) -> Self {
        Self {
            code: "invalid_configuration".into(),
            message,
            location: Box::default(),
            line: None,
            column: None,
        }
    }

    pub(crate) fn code(mut self, code: &str) -> Self {
        self.code = code.into();
        self
    }
    pub(crate) fn at(mut self, path: impl Into<String>) -> Self {
        self.location.path = path.into();
        self
    }
    pub(crate) fn device(mut self, id: &str, field: &str) -> Self {
        self.location.device_id = Some(id.into());
        self.at(field)
    }
    pub(crate) fn port(mut self, id: &str, field: &str) -> Self {
        self.location.port_id = Some(id.into());
        self.at(field)
    }
    pub(crate) fn link(mut self, id: &str, field: &str) -> Self {
        self.location.link_id = Some(id.into());
        self.at(field)
    }
    pub(crate) fn probe(mut self, id: &str, field: &str) -> Self {
        self.location.probe_id = Some(id.into());
        self.at(field)
    }

    pub(crate) fn locate(mut self, input: &str) -> Self {
        // Resolve source positions only on failure; compiling valid inputs needs no second tree.
        let Ok(doc) = serde_json::from_str::<Value>(input.trim_start_matches('\u{feff}')) else {
            return self;
        };
        if !self.location.path.starts_with('/') {
            let mut base = String::new();
            for (index, device) in doc["devices"].as_array().into_iter().flatten().enumerate() {
                if self
                    .location
                    .device_id
                    .as_ref()
                    .is_some_and(|id| device["id"].as_str() != Some(id.as_str()))
                {
                    continue;
                }
                if let Some(id) = &self.location.port_id {
                    let host = device["port"]["id"].as_str() == Some(id.as_str());
                    let port = device["ports"].as_array().and_then(|ports| {
                        ports
                            .iter()
                            .rposition(|port| port["id"].as_str() == Some(id.as_str()))
                    });
                    if host || port.is_some() {
                        self.location.device_id = device["id"].as_str().map(str::to_owned);
                        base = if host
                            && ["ipv4", "prefix_length", "mac"]
                                .contains(&self.location.path.as_str())
                        {
                            format!("/devices/{index}")
                        } else if host {
                            format!("/devices/{index}/port")
                        } else {
                            format!("/devices/{index}/ports/{}", port.unwrap())
                        };
                        break;
                    }
                } else if device["id"].as_str() == self.location.device_id.as_deref()
                    && self.location.device_id.is_some()
                {
                    base = format!("/devices/{index}");
                    break;
                }
            }
            for (key, id) in [
                ("links", &self.location.link_id),
                ("scenario/probes", &self.location.probe_id),
            ] {
                if let Some(id) = id {
                    if let Some(index) = doc
                        .pointer(&format!("/{key}"))
                        .and_then(Value::as_array)
                        .and_then(|rows| {
                            rows.iter()
                                .position(|row| row["id"].as_str() == Some(id.as_str()))
                        })
                    {
                        base = format!("/{key}/{index}");
                    } else if key == "scenario/probes" && doc.get("probe").is_some() {
                        base = "/probe".into();
                    }
                }
            }
            self.location.path = if self.location.path.is_empty() {
                base
            } else {
                format!("{base}/{}", self.location.path)
            };
        }
        // Typed decoding can identify a containing object even when no stable ID was read yet.
        let segments: Vec<_> = self.location.path.split('/').collect();
        if segments.get(1) == Some(&"devices")
            && let Some(index) = segments
                .get(2)
                .and_then(|index| index.parse::<usize>().ok())
        {
            let device = &doc["devices"][index];
            self.location.device_id = device["id"].as_str().map(str::to_owned);
            let port = if segments.get(3) == Some(&"port") {
                &device["port"]
            } else if segments.get(3) == Some(&"ports")
                && let Some(port) = segments.get(4).and_then(|v| v.parse::<usize>().ok())
            {
                &device["ports"][port]
            } else {
                &Value::Null
            };
            if let Some(id) = port["id"].as_str() {
                self.location.port_id = Some(id.into());
            }
        }
        if segments.get(1) == Some(&"links")
            && let Some(index) = segments
                .get(2)
                .and_then(|index| index.parse::<usize>().ok())
        {
            self.location.link_id = doc["links"][index]["id"].as_str().map(str::to_owned);
        }
        if segments.get(1) == Some(&"scenario")
            && segments.get(2) == Some(&"probes")
            && let Some(index) = segments
                .get(3)
                .and_then(|index| index.parse::<usize>().ok())
        {
            self.location.probe_id = doc["scenario"]["probes"][index]["id"]
                .as_str()
                .map(str::to_owned);
        }
        self
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.message)
    }
}
impl std::error::Error for InputError {}

pub(crate) fn decode<T: DeserializeOwned>(input: &str) -> Result<T, InputError> {
    // Track field paths only for rejected inputs; successful compilation keeps the direct parser.
    let original = match serde_json::from_str(input) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let mut decoder = serde_json::Deserializer::from_str(input);
    let tracked: Result<T, _> = serde_path_to_error::deserialize(&mut decoder);
    let mut pointer = String::new();
    if let Err(error) = tracked {
        for segment in error.path() {
            let key = match segment {
                serde_path_to_error::Segment::Seq { index } => index.to_string(),
                serde_path_to_error::Segment::Map { key } => key.clone(),
                _ => continue,
            };
            pointer.push('/');
            pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
        }
    }
    // from_str above also checked trailing content; a successful tracked value cannot admit it.
    Err(json_error(original).at(pointer))
}

fn json_error(error: serde_json::Error) -> InputError {
    let mut result = InputError::new(format!("实验 JSON 无法读取。{error}")).code(
        if error.is_syntax() || error.is_eof() {
            "invalid_json"
        } else {
            "invalid_value"
        },
    );
    result.line = (error.line() != 0).then_some(error.line());
    result.column = (error.column() != 0).then_some(error.column());
    result
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ValidationSummary {
    pub topology_id: String,
    pub revision: String,
    pub format_version: u32,
    pub devices: usize,
    pub ports: usize,
    pub links: usize,
    pub probes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NativeFormat {
    Supported { format_version: u32 },
    Unsupported { format_version: u32 },
    Invalid { error: InputError },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativePreview {
    pub text: String,
    pub format: NativeFormat,
}

pub fn preview_native(text: String) -> NativePreview {
    #[derive(Deserialize)]
    struct Version {
        format_version: u32,
    }
    let format = match decode::<Version>(text.trim_start_matches('\u{feff}')) {
        Ok(Version {
            format_version: version @ 1..=3,
        }) => NativeFormat::Supported {
            format_version: version,
        },
        Ok(Version { format_version }) => NativeFormat::Unsupported { format_version },
        Err(error) => NativeFormat::Invalid { error },
    };
    NativePreview { text, format }
}

impl From<Topology> for ValidationSummary {
    fn from(topology: Topology) -> Self {
        Self {
            topology_id: topology.id,
            revision: topology.revision,
            format_version: topology.format_version,
            devices: topology.devices.len(),
            ports: topology.ports.len(),
            links: topology.links.len(),
            probes: topology.probes.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ValidationResult {
    Valid { summary: ValidationSummary },
    Invalid { error: InputError },
}

impl From<Result<ValidationSummary, InputError>> for ValidationResult {
    fn from(result: Result<ValidationSummary, InputError>) -> Self {
        match result {
            Ok(summary) => Self::Valid { summary },
            Err(error) => Self::Invalid { error },
        }
    }
}

pub fn validate_lab(input: &str) -> Result<ValidationSummary, InputError> {
    Topology::from_json(input, false).map(ValidationSummary::from)
}

pub(crate) fn compile_for_workspace(input: &str) -> Result<Topology, InputError> {
    let topology = Topology::from_json(input, false)?;
    topology
        .validate_workspace_labels()
        .map_err(|error| error.locate(input))?;
    Ok(topology)
}

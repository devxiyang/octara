//! Host-independent accessibility roles, properties, and actions.

use crate::identity::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Window,
    #[default]
    GenericContainer,
    Label,
    Heading,
    Paragraph,
    Blockquote,
    Code,
    Button,
    CheckBox,
    RadioButton,
    RadioGroup,
    Switch,
    TextInput,
    MultilineTextInput,
    PasswordInput,
    ComboBox,
    EditableComboBox,
    List,
    ListItem,
    ListBox,
    ListBoxOption,
    Table,
    Row,
    Cell,
    ColumnHeader,
    Tree,
    TreeItem,
    Menu,
    MenuItem,
    MenuItemCheckbox,
    MenuItemRadio,
    Tab,
    TabList,
    TabPanel,
    Dialog,
    AlertDialog,
    Status,
    Alert,
    Main,
    Navigation,
    Separator,
    Tooltip,
    Toolbar,
    Group,
    Document,
    Link,
    Image,
    Slider,
    SpinButton,
    ProgressIndicator,
    WebView,
    ScrollView,
    Splitter,
}

/// The state of a checkable control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checked {
    #[default]
    Unchecked,
    Checked,
    Mixed,
}

impl From<bool> for Checked {
    fn from(checked: bool) -> Self {
        if checked { Self::Checked } else { Self::Unchecked }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Live {
    Off,
    Polite,
    Assertive,
}

/// Authored references or their resolved node handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relations<T> {
    pub active_descendant: Option<T>,
    pub labelled_by: Option<T>,
    pub described_by: Option<T>,
    pub controls: Option<T>,
}

impl<T> Default for Relations<T> {
    fn default() -> Self {
        Self { active_descendant: None, labelled_by: None, described_by: None, controls: None }
    }
}

impl<T> Relations<T> {
    pub fn is_empty(&self) -> bool {
        self.active_descendant.is_none() && self.labelled_by.is_none() && self.described_by.is_none() && self.controls.is_none()
    }
}

/// Numeric bounds for controls such as sliders and progress indicators.
/// An absent value represents an unknown number, such as indeterminate progress.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumericRange {
    pub value: Option<f64>,
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
}

/// A logical UTF-8 byte position supplied by an accessibility adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPosition {
    pub node: NodeId,
    pub offset: usize,
}

/// An assistive technology request expressed in the retained tree's terms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A11yAction {
    Press,
    Focus,
    SetValue(String),
    SetNumericValue(f64),
    Increment,
    Decrement,
    ShowContextMenu,
    SetTextSelection { anchor: TextPosition, focus: TextPosition },
    CopyTextSelection,
}

/// A text limit observed by a host after layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextClip {
    Ellipsis,
    LineClamp(u32),
}

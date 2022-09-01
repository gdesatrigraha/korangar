use std::cell::RefCell;
use std::rc::{Rc, Weak};

use num::Zero;
use procedural::*;

use crate::graphics::{Color, InterfaceRenderer, Renderer};
use crate::input::UserEvent;
use crate::interface::{Element, *};
use crate::network::CharacterInformation;

// TODO: rework all of this
pub struct CharacterPreview {
    characters: Rc<RefCell<Vec<CharacterInformation>>>,
    move_request: Rc<RefCell<Option<usize>>>,
    changed: Rc<RefCell<bool>>,
    slot: usize,
    state: ContainerState,
}

impl CharacterPreview {

    fn get_elements(
        characters: &Rc<RefCell<Vec<CharacterInformation>>>,
        move_request: &Rc<RefCell<Option<usize>>>,
        slot: usize,
    ) -> Vec<ElementCell> {

        if let Some(origin_slot) = *move_request.borrow() {

            let text = match origin_slot == slot {
                true => "click to cancel",
                false => "switch",
            };

            return vec![cell!(Text::new(
                text.to_string(),
                Color::rgb(200, 140, 180),
                14.0,
                constraint!(100%, 14)
            ))];
        }

        let characters = characters.borrow();
        let character_information = characters.iter().find(|character| character.character_number as usize == slot);

        if let Some(character_information) = character_information {

            return vec![
                cell!(Text::new(
                    character_information.name.clone(),
                    Color::rgb(220, 210, 210),
                    18.0,
                    constraint!(100%, 18)
                )), // alignment!(center, top)
                cell!(EventButton::new(
                    "switch slot".to_string(),
                    UserEvent::RequestSwitchCharacterSlot(slot)
                )),
                cell!(EventButton::new(
                    "delete character".to_string(),
                    UserEvent::DeleteCharacter(character_information.character_id as usize)
                )),
            ];
        }

        vec![cell!(Text::new(
            "new character".to_string(),
            Color::rgb(200, 140, 180),
            14.0,
            constraint!(100%, 14)
        ))]
    }

    pub fn new(
        characters: Rc<RefCell<Vec<CharacterInformation>>>,
        move_request: Rc<RefCell<Option<usize>>>,
        changed: Rc<RefCell<bool>>,
        slot: usize,
    ) -> Self {

        let elements = Self::get_elements(&characters, &move_request, slot);
        let state = ContainerState::new(elements);

        Self {
            characters,
            move_request,
            changed,
            slot,
            state,
        }
    }

    fn has_character(&self) -> bool {
        self.state.elements.len() > 1 // TODO:
    }
}

impl Element for CharacterPreview {

    fn get_state(&self) -> &ElementState {
        &self.state.state
    }

    fn get_state_mut(&mut self) -> &mut ElementState {
        &mut self.state.state
    }

    fn link_back(&mut self, weak_self: Weak<RefCell<dyn Element>>, weak_parent: Option<Weak<RefCell<dyn Element>>>) {
        self.state.link_back(weak_self, weak_parent);
    }

    fn resolve(&mut self, placement_resolver: &mut PlacementResolver, interface_settings: &InterfaceSettings, theme: &Theme) {

        let size_constraint = &constraint!(20%, 150);
        self.state.resolve(placement_resolver, interface_settings, theme, size_constraint);
    }

    fn update(&mut self) -> Option<ChangeEvent> {

        if !*self.changed.borrow() {
            return None;
        }

        *self = Self::new(
            self.characters.clone(),
            self.move_request.clone(),
            self.changed.clone(),
            self.slot,
        );

        Some(ChangeEvent::Reresolve)
    }

    fn left_click(&mut self, _update: &mut bool) -> Option<ClickAction> {

        if let Some(origin_slot) = *self.move_request.borrow() {

            let event = match origin_slot == self.slot {
                true => UserEvent::CancelSwitchCharacterSlot,
                false => UserEvent::SwitchCharacterSlot(self.slot),
            };

            return Some(ClickAction::Event(event));
        }

        let event = match self.has_character() {
            true => UserEvent::SelectCharacter(self.slot),
            false => UserEvent::OpenCharacterCreationWindow(self.slot),
        };

        Some(ClickAction::Event(event))
    }

    fn hovered_element(&self, mouse_position: Position) -> HoverInformation {
        self.state.hovered_element::<true>(mouse_position)
    }

    fn render(
        &self,
        render_target: &mut <InterfaceRenderer as Renderer>::Target,
        renderer: &InterfaceRenderer,
        state_provider: &StateProvider,
        interface_settings: &InterfaceSettings,
        theme: &Theme,
        parent_position: Position,
        clip_size: ClipSize,
        hovered_element: Option<&dyn Element>,
        focused_element: Option<&dyn Element>,
        second_theme: bool,
    ) {

        let mut renderer = self
            .state
            .state
            .element_renderer(render_target, renderer, interface_settings, parent_position, clip_size);

        let background_color = match self.is_element_self(hovered_element) {
            true => *theme.button.hovered_background_color,
            false => *theme.button.background_color,
        };

        renderer.render_background(*theme.button.border_radius, background_color);

        self.state.render(
            &mut renderer,
            state_provider,
            interface_settings,
            theme,
            hovered_element,
            focused_element,
            second_theme,
        );
    }
}

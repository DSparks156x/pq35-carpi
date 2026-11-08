//! Readings: a title and a 2x4 grid of labelled values. A value change re-sends
//! one padded text run; nothing is cleared.
//!
//! Layout: title y=0; rows y=12/21/30/39; left column label 0..10, value
//! 10..31; separator fill at x=32; right column label 33..43, value 43..64.
//! A value field is 42 px: four proportional digits (10 px each) fit.
//! The separator is a 1-wide `52` fill, not a `63` line, so it needs no ROP latch.

use super::Screen;
use crate::cmd::Op;
use crate::font::{Font, audscii};
use crate::helpers::{Ctx, NumberField, TextSlot};
use crate::pack::Group;
use crate::proto::Rect;
use std::time::Instant;

pub const ROWS: [u8; 4] = [12, 21, 30, 39];
pub const SEPARATOR: Rect = Rect::new(32, 11, 1, 37);

/// One cell's config: which store value it shows and how.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingSpec {
    /// Store name, e.g. `engine.oil_temp_c`. The caller reads it; the screen doesn't.
    pub key: String,
    pub label: String,
    pub decimals: u8,
}

/// Inputs: values in spec order. `None` shows `--`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadingsInput {
    pub title: String,
    pub values: [Option<f64>; 8],
}

pub struct ReadingsScreen {
    pub specs: Vec<ReadingSpec>,
    input: ReadingsInput,
    title: TextSlot,
    labels: Vec<TextSlot>,
    values: Vec<NumberField>,
}

fn cell(i: usize) -> (u8, u8, u8) {
    let (col, row) = (i / 4, i % 4);
    let y = ROWS[row];
    if col == 0 { (y, 0, 10) } else { (y, 33, 43) }
}

pub fn format_value(v: Option<f64>, decimals: u8) -> String {
    match v.filter(|v| v.is_finite()) {
        Some(v) => format!("{:.*}", usize::from(decimals), v),
        None => "--".into(),
    }
}

impl ReadingsScreen {
    /// `specs` beyond 8 are ignored; fewer leave cells empty.
    pub fn new(specs: Vec<ReadingSpec>) -> Self {
        let mut labels = Vec::new();
        let mut values = Vec::new();
        for i in 0..8 {
            let (y, l, v) = cell(i);
            labels.push(TextSlot::new(y, l, v, Font::Prop));
            let right = if i < 4 { 31 } else { 64 };
            values.push(NumberField::new(y, v, right, Font::Prop));
        }
        ReadingsScreen {
            specs,
            input: ReadingsInput::default(),
            title: TextSlot::new(0, 0, 64, Font::Prop),
            labels,
            values,
        }
    }

    pub fn set(&mut self, input: ReadingsInput) {
        self.input = input;
    }

    fn value_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let mut out = Vec::new();
        for (i, f) in self.values.iter_mut().enumerate() {
            let Some(spec) = self.specs.get(i) else {
                continue;
            };
            let t = format_value(self.input.values[i], spec.decimals);
            out.extend(f.set(cx, "value", &t));
        }
        out
    }

    fn title_group(&mut self, cx: &Ctx) -> Option<Group> {
        let m = &cx.metrics;
        let b = audscii(&self.input.title);
        let n = m
            .fit(Font::Prop, &b, self.title.width_px())
            .min(cx.text_cap());
        self.title.draw(m, "title", 0, b[..n].to_vec())
    }
}

impl Screen for ReadingsScreen {
    fn enter(&mut self, _now: Instant, cx: &Ctx) -> Vec<Group> {
        self.title.reset();
        self.labels.iter_mut().for_each(TextSlot::reset);
        self.values.iter_mut().for_each(NumberField::reset);
        let mut out: Vec<Group> = self.title_group(cx).into_iter().collect();
        out.push(Group::new("separator", vec![Op::Fill(SEPARATOR)]));
        let m = cx.metrics;
        for (i, slot) in self.labels.iter_mut().enumerate() {
            let Some(spec) = self.specs.get(i) else {
                continue;
            };
            let b = audscii(&spec.label);
            let n = m.fit(Font::Prop, &b, slot.width_px());
            let x = slot.left;
            out.extend(slot.draw(&m, "label", x, b[..n].to_vec()));
        }
        out.extend(self.value_groups(cx));
        out
    }

    fn update(&mut self, _now: Instant, cx: &Ctx) -> Vec<Group> {
        let mut out: Vec<Group> = self.title_group(cx).into_iter().collect();
        out.extend(self.value_groups(cx));
        out
    }
}

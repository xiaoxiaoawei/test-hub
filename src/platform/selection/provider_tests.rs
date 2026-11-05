//! In-process UIA client interfaces exercise the actual range-reading contract.
//! They are test doubles, not a substitute for a Windows browser/PDF test.
#![allow(non_snake_case, unused_variables)]

use super::{probe::Gesture, selected_text, MAX_SELECTION_UNITS};
use std::{
    ptr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{implement, Error, Ref, Result, BOOL, BSTR},
    Win32::{
        Foundation::{E_NOTIMPL, POINT},
        System::{
            Com::SAFEARRAY,
            Ole::{SafeArrayAccessData, SafeArrayCreateVector, SafeArrayUnaccessData},
            Variant::{VARIANT, VT_R8},
        },
        UI::Accessibility::*,
    },
};

#[derive(Default)]
struct Calls {
    selection: AtomicUsize,
    text: AtomicUsize,
    forbidden: AtomicUsize,
}

#[implement(IUIAutomationTextRange)]
struct Range {
    text: String,
    bounds: Vec<f64>,
    calls: Arc<Calls>,
}
impl Range_Impl {
    fn forbidden<T>(&self) -> Result<T> {
        self.calls.forbidden.fetch_add(1, Ordering::SeqCst);
        Err(Error::from_hresult(E_NOTIMPL))
    }
}
impl IUIAutomationTextRange_Impl for Range_Impl {
    fn Clone(&self) -> Result<IUIAutomationTextRange> {
        self.forbidden()
    }
    fn Compare(&self, range: Ref<'_, IUIAutomationTextRange>) -> Result<BOOL> {
        self.forbidden()
    }
    fn CompareEndpoints(
        &self,
        srcendpoint: TextPatternRangeEndpoint,
        range: Ref<'_, IUIAutomationTextRange>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> Result<i32> {
        self.forbidden()
    }
    fn ExpandToEnclosingUnit(&self, textunit: TextUnit) -> Result<()> {
        self.forbidden()
    }
    fn FindAttribute(
        &self,
        attr: UIA_TEXTATTRIBUTE_ID,
        val: &VARIANT,
        backward: BOOL,
    ) -> Result<IUIAutomationTextRange> {
        self.forbidden()
    }
    fn FindText(
        &self,
        text: &BSTR,
        backward: BOOL,
        ignorecase: BOOL,
    ) -> Result<IUIAutomationTextRange> {
        self.forbidden()
    }
    fn GetAttributeValue(&self, attr: UIA_TEXTATTRIBUTE_ID) -> Result<VARIANT> {
        self.forbidden()
    }
    fn GetEnclosingElement(&self) -> Result<IUIAutomationElement> {
        self.forbidden()
    }
    fn Move(&self, unit: TextUnit, count: i32) -> Result<i32> {
        self.forbidden()
    }
    fn MoveEndpointByUnit(
        &self,
        endpoint: TextPatternRangeEndpoint,
        unit: TextUnit,
        count: i32,
    ) -> Result<i32> {
        self.forbidden()
    }
    fn MoveEndpointByRange(
        &self,
        srcendpoint: TextPatternRangeEndpoint,
        range: Ref<'_, IUIAutomationTextRange>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> Result<()> {
        self.forbidden()
    }
    fn Select(&self) -> Result<()> {
        self.forbidden()
    }
    fn AddToSelection(&self) -> Result<()> {
        self.forbidden()
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        self.forbidden()
    }
    fn ScrollIntoView(&self, aligntotop: BOOL) -> Result<()> {
        self.forbidden()
    }
    fn GetChildren(&self) -> Result<IUIAutomationElementArray> {
        self.forbidden()
    }
    fn GetBoundingRectangles(&self) -> Result<*mut SAFEARRAY> {
        if self.bounds.is_empty() {
            return Err(Error::from_hresult(E_NOTIMPL));
        }
        unsafe {
            let array = SafeArrayCreateVector(VT_R8, 0, self.bounds.len() as u32);
            assert!(!array.is_null());
            let mut data = ptr::null_mut();
            SafeArrayAccessData(array, &mut data)?;
            ptr::copy_nonoverlapping(self.bounds.as_ptr(), data.cast::<f64>(), self.bounds.len());
            SafeArrayUnaccessData(array)?;
            Ok(array)
        }
    }
    fn GetText(&self, maxlength: i32) -> Result<BSTR> {
        self.calls.text.fetch_add(1, Ordering::SeqCst);
        assert_eq!(maxlength, (MAX_SELECTION_UNITS + 1) as i32);
        let units: Vec<u16> = self.text.encode_utf16().take(maxlength as usize).collect();
        Ok(BSTR::from_wide(&units))
    }
}

#[implement(IUIAutomationTextRangeArray)]
struct Ranges {
    values: Vec<IUIAutomationTextRange>,
}
impl IUIAutomationTextRangeArray_Impl for Ranges_Impl {
    fn Length(&self) -> Result<i32> {
        Ok(self.values.len() as i32)
    }
    fn GetElement(&self, index: i32) -> Result<IUIAutomationTextRange> {
        self.values
            .get(index as usize)
            .cloned()
            .ok_or_else(|| Error::from_hresult(E_NOTIMPL))
    }
}

#[implement(IUIAutomationTextPattern)]
struct Pattern {
    ranges: IUIAutomationTextRangeArray,
    supported: SupportedTextSelection,
    calls: Arc<Calls>,
}
impl Pattern_Impl {
    fn forbidden<T>(&self) -> Result<T> {
        self.calls.forbidden.fetch_add(1, Ordering::SeqCst);
        Err(Error::from_hresult(E_NOTIMPL))
    }
}
impl IUIAutomationTextPattern_Impl for Pattern_Impl {
    fn RangeFromPoint(&self, pt: &POINT) -> Result<IUIAutomationTextRange> {
        self.forbidden()
    }
    fn RangeFromChild(
        &self,
        child: Ref<'_, IUIAutomationElement>,
    ) -> Result<IUIAutomationTextRange> {
        self.forbidden()
    }
    fn GetSelection(&self) -> Result<IUIAutomationTextRangeArray> {
        self.calls.selection.fetch_add(1, Ordering::SeqCst);
        Ok(self.ranges.clone())
    }
    fn GetVisibleRanges(&self) -> Result<IUIAutomationTextRangeArray> {
        self.forbidden()
    }
    fn DocumentRange(&self) -> Result<IUIAutomationTextRange> {
        self.forbidden()
    }
    fn SupportedTextSelection(&self) -> Result<SupportedTextSelection> {
        Ok(self.supported)
    }
}

fn pattern(
    values: &[(&str, &[f64])],
    supported: SupportedTextSelection,
) -> (IUIAutomationTextPattern, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let values = values
        .iter()
        .map(|(text, bounds)| {
            Range {
                text: (*text).into(),
                bounds: bounds.to_vec(),
                calls: calls.clone(),
            }
            .into()
        })
        .collect();
    let ranges: IUIAutomationTextRangeArray = Ranges { values }.into();
    (
        Pattern {
            ranges,
            supported,
            calls: calls.clone(),
        }
        .into(),
        calls,
    )
}

fn read(pattern: &IUIAutomationTextPattern, gesture: Option<Gesture>) -> Option<String> {
    unsafe { selected_text(pattern, gesture, Instant::now() + Duration::from_secs(1)) }.unwrap()
}

#[test]
fn real_range_reader_rejects_address_bar_geometry_before_get_text() {
    let (pattern, calls) = pattern(
        &[("stale address bar", &[100.0, 25.0, 400.0, 20.0])],
        SupportedTextSelection_Single,
    );
    assert_eq!(
        read(
            &pattern,
            Some(Gesture {
                origin: Some((100, 240)),
                end: (270, 290)
            })
        ),
        None
    );
    assert_eq!(calls.text.load(Ordering::SeqCst), 0);
    assert_eq!(calls.forbidden.load(Ordering::SeqCst), 0);
}

#[test]
fn actual_selected_ranges_preserve_unicode_and_never_read_document_range() {
    let (pattern, calls) = pattern(
        &[
            ("Hello 世界 🦀", &[100.0, 230.0, 230.0, 20.0]),
            ("second line", &[100.0, 254.0, 230.0, 20.0]),
        ],
        SupportedTextSelection_Multiple,
    );
    assert_eq!(
        read(
            &pattern,
            Some(Gesture {
                origin: Some((100, 240)),
                end: (270, 270)
            })
        )
        .as_deref(),
        Some("Hello 世界 🦀\nsecond line")
    );
    assert_eq!(calls.text.load(Ordering::SeqCst), 2);
    assert_eq!(calls.forbidden.load(Ordering::SeqCst), 0);
}

#[test]
fn unsupported_selection_is_not_read_as_a_document_or_caret() {
    let (pattern, calls) = pattern(&[("must never be read", &[])], SupportedTextSelection_None);
    assert_eq!(read(&pattern, None), None);
    assert_eq!(calls.selection.load(Ordering::SeqCst), 0);
    assert_eq!(calls.text.load(Ordering::SeqCst), 0);
    assert_eq!(calls.forbidden.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_range_geometry_uses_the_already_verified_point_scope() {
    let (pattern, calls) = pattern(
        &[("PDF provider without range rectangles", &[])],
        SupportedTextSelection_Single,
    );
    assert!(read(
        &pattern,
        Some(Gesture {
            origin: Some((100, 240)),
            end: (270, 270)
        })
    )
    .is_some());
    assert_eq!(calls.text.load(Ordering::SeqCst), 1);
    assert_eq!(calls.forbidden.load(Ordering::SeqCst), 0);
}

#[test]
fn range_and_utf16_limits_are_enforced_without_partial_text() {
    let values = vec![("x", &[][..]); 33];
    let (many, calls) = pattern(&values, SupportedTextSelection_Multiple);
    assert!(
        unsafe { selected_text(&many, None, Instant::now() + Duration::from_secs(1)) }.is_err()
    );
    assert_eq!(calls.text.load(Ordering::SeqCst), 0);
    let oversized = "x".repeat(MAX_SELECTION_UNITS + 1);
    let (large, calls) = pattern(&[(&oversized, &[])], SupportedTextSelection_Single);
    assert!(
        unsafe { selected_text(&large, None, Instant::now() + Duration::from_secs(1)) }.is_err()
    );
    assert_eq!(calls.text.load(Ordering::SeqCst), 1);
    assert_eq!(calls.forbidden.load(Ordering::SeqCst), 0);
}

#[test]
fn expired_capture_never_touches_the_provider() {
    let (pattern, calls) = pattern(&[("expired", &[])], SupportedTextSelection_Single);
    assert!(unsafe { selected_text(&pattern, None, Instant::now()) }
        .unwrap()
        .is_none());
    assert_eq!(calls.selection.load(Ordering::SeqCst), 0);
    assert_eq!(calls.text.load(Ordering::SeqCst), 0);
}

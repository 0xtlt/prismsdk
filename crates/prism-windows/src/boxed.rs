//! A delegate boxed as an `IReference<T>`, the way XAML takes the handler
//! of `UIElement::AddHandler` (C++/WinRT's `box_value`, C#'s delegates):
//! how a control's own handled events (a text box's presses) still reach
//! Prism (`handledEventsToo`).

use windows::Foundation::{
    DateTime, IPropertyValue, IPropertyValue_Impl, IReference, IReference_Impl, Point,
    PropertyType, Rect, Size, TimeSpan,
};
use windows::Win32::Foundation::E_NOTIMPL;
use windows_core::{Array, Error, GUID, HSTRING, IInspectable, Result, RuntimeType, implement};

#[implement(IReference<T>, IPropertyValue)]
struct Boxed<T>(T)
where
    T: RuntimeType + Clone + 'static;

/// `value` (a delegate) as an `IInspectable` holding it.
pub fn boxed<T: RuntimeType + Clone + 'static>(value: T) -> IInspectable {
    Boxed(value).into()
}

impl<T: RuntimeType + Clone + 'static> IReference_Impl<T> for Boxed_Impl<T> {
    fn Value(&self) -> Result<T> {
        Ok(self.0.clone())
    }
}

fn none<R>() -> Result<R> {
    Err(Error::from(E_NOTIMPL))
}

/// What `IPropertyValue` asks of a box that is not a value: none of it.
macro_rules! not_values {
    ($($get:ident -> $t:ty),* ; $($array:ident: $a:ty),*) => {
        $(fn $get(&self) -> Result<$t> { none() })*
        $(fn $array(&self, _: &mut Array<$a>) -> Result<()> { none() })*
    };
}

impl<T: RuntimeType + Clone + 'static> IPropertyValue_Impl for Boxed_Impl<T> {
    fn Type(&self) -> Result<PropertyType> {
        Ok(PropertyType::OtherType)
    }
    fn IsNumericScalar(&self) -> Result<bool> {
        Ok(false)
    }
    not_values!(
        GetUInt8 -> u8, GetInt16 -> i16, GetUInt16 -> u16, GetInt32 -> i32,
        GetUInt32 -> u32, GetInt64 -> i64, GetUInt64 -> u64, GetSingle -> f32,
        GetDouble -> f64, GetChar16 -> u16, GetBoolean -> bool, GetString -> HSTRING,
        GetGuid -> GUID, GetDateTime -> DateTime, GetTimeSpan -> TimeSpan,
        GetPoint -> Point, GetSize -> Size, GetRect -> Rect;
        GetUInt8Array: u8, GetInt16Array: i16, GetUInt16Array: u16, GetInt32Array: i32,
        GetUInt32Array: u32, GetInt64Array: i64, GetUInt64Array: u64,
        GetSingleArray: f32, GetDoubleArray: f64, GetChar16Array: u16,
        GetBooleanArray: bool, GetStringArray: HSTRING, GetInspectableArray: IInspectable,
        GetGuidArray: GUID, GetDateTimeArray: DateTime, GetTimeSpanArray: TimeSpan,
        GetPointArray: Point, GetSizeArray: Size, GetRectArray: Rect
    );
}

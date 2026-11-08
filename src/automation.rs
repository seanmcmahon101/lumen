//! Small, typed ownership boundary around COM Automation used by the mail helper.
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::*;
use windows::core::{GUID, HSTRING, Interface, PCWSTR, Result};

pub struct Apartment;
impl Apartment {
    pub fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

pub struct Object(IDispatch);
impl Object {
    pub fn create(name: &str) -> Result<Self> {
        unsafe {
            let clsid = CLSIDFromProgID(&HSTRING::from(name))?;
            Ok(Self(CoCreateInstance(
                &clsid,
                None,
                CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER,
            )?))
        }
    }
    pub fn active(name: &str) -> Result<Self> {
        unsafe {
            let clsid = CLSIDFromProgID(&HSTRING::from(name))?;
            let mut unknown = None;
            windows::Win32::System::Ole::GetActiveObject(&clsid, None, &mut unknown)?;
            Ok(Self(
                unknown
                    .ok_or_else(windows::core::Error::from_thread)?
                    .cast()?,
            ))
        }
    }
    fn invoke(&self, name: &str, flags: DISPATCH_FLAGS, args: &[VARIANT]) -> Result<VARIANT> {
        let wide = HSTRING::from(name);
        let name = PCWSTR(wide.as_ptr());
        let mut id = 0;
        let mut values: Vec<_> = args.iter().rev().cloned().collect();
        let mut put = -3i32;
        let params = DISPPARAMS {
            rgvarg: values.as_mut_ptr(),
            cArgs: values.len() as u32,
            rgdispidNamedArgs: if flags == DISPATCH_PROPERTYPUT {
                &mut put
            } else {
                std::ptr::null_mut()
            },
            cNamedArgs: u32::from(flags == DISPATCH_PROPERTYPUT),
        };
        let mut result = VARIANT::default();
        unsafe {
            self.0
                .GetIDsOfNames(&GUID::zeroed(), &name, 1, 0, &mut id)?;
            self.0.Invoke(
                id,
                &GUID::zeroed(),
                0,
                flags,
                &params,
                Some(&mut result),
                None,
                None,
            )?;
        }
        Ok(result)
    }
    pub fn get(&self, name: &str) -> Result<VARIANT> {
        self.invoke(name, DISPATCH_PROPERTYGET, &[])
    }
    pub fn call(&self, name: &str, args: &[VARIANT]) -> Result<VARIANT> {
        self.invoke(name, DISPATCH_METHOD, args)
    }
    pub fn object(&self, name: &str, args: &[VARIANT]) -> Result<Self> {
        Ok(Self(IDispatch::try_from(&self.invoke(
            name,
            DISPATCH_PROPERTYGET,
            args,
        )?)?))
    }
    pub fn called(&self, name: &str, args: &[VARIANT]) -> Result<Self> {
        Ok(Self(IDispatch::try_from(&self.call(name, args)?)?))
    }
    pub fn put(&self, name: &str, value: VARIANT) -> Result<()> {
        self.invoke(name, DISPATCH_PROPERTYPUT, &[value])
            .map(|_| ())
    }
    pub fn text(&self, name: &str) -> String {
        self.get(name).map(|value| text(&value)).unwrap_or_default()
    }
    pub fn number(&self, name: &str) -> i32 {
        self.get(name)
            .ok()
            .and_then(|value| i32::try_from(&value).ok())
            .unwrap_or_default()
    }
}
pub fn text(value: &VARIANT) -> String {
    let mut converted = VARIANT::default();
    if unsafe { VariantChangeType(&mut converted, value, VAR_CHANGE_FLAGS(0), VT_BSTR) }.is_err() {
        return String::new();
    }
    unsafe { converted.Anonymous.Anonymous.Anonymous.bstrVal.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_automation_handles_properties_arguments_and_values() {
        let _com = Apartment::new().unwrap();
        let dictionary = Object::create("Scripting.Dictionary").unwrap();
        dictionary.put("CompareMode", 1i32.into()).unwrap();
        dictionary
            .call("Add", &["Invoice".into(), 42i32.into()])
            .unwrap();
        assert_eq!(dictionary.number("Count"), 1);
        assert!(bool::try_from(&dictionary.call("Exists", &["invoice".into()]).unwrap()).unwrap());
        let value = dictionary
            .invoke("Item", DISPATCH_PROPERTYGET, &["Invoice".into()])
            .unwrap();
        assert_eq!(text(&value), "42");
    }
}

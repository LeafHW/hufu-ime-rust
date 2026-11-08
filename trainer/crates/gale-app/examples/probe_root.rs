#[path = "../src/qq_bridge.rs"]
mod qq_bridge;
#[path = "../src/uia_bridge.rs"]
mod uia_bridge;

use windows::core::{Interface, VARIANT};
use windows::Win32::UI::Accessibility::*;

fn main() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeSecurity(
            None,
            -1,
            None,
            None,
            windows::Win32::System::Com::RPC_C_AUTHN_LEVEL_DEFAULT,
            windows::Win32::System::Com::RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            windows::Win32::System::Com::EOAC_NONE,
            None,
        );
        let _ = windows::Win32::System::Ole::OleInitialize(None);
        let uia: IUIAutomation = windows::Win32::System::Com::CoCreateInstance(
            &CUIAutomation,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )
        .unwrap();
        let root = uia.GetRootElement().unwrap();
        let cond = uia
            .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from("QQ"))
            .unwrap();
        match root.FindFirst(TreeScope_Children, &cond) {
            Ok(el) => {
                println!("FOUND {:?}", el.CurrentClassName().map(|s| s.to_string()));
                let all = el
                    .FindAll(TreeScope_Descendants, &uia.CreateTrueCondition().unwrap())
                    .unwrap();
                println!("DESC {}", all.Length().unwrap());
            }
            Err(e) => println!("NOFIND {e:?}"),
        }
    }
}

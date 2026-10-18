//! ACL 层（CONTRACTS.md acl.rs 契约，windows 0.58）。
//! 血律对照：install.ps1 Set-Reg/Set-RegHKLM/icacls —— 具体为 L100-101、L120-121：
//!   `icacls <dir> /grant 'ALL APPLICATION PACKAGES:(OI)(CI)RX'`（inherit=true，目录）
//!   `icacls <dll> /grant 'ALL APPLICATION PACKAGES:RX'`（inherit=false，文件）
//! 流程：GetNamedSecurityInfoW(DACL) → ConvertStringSidToSidW("S-1-15-2-1") →
//! EXPLICIT_ACCESS_W{GRANT_ACCESS, GENERIC_READ|GENERIC_EXECUTE,
//! (OI)(CI)=SUB_CONTAINERS_AND_OBJECTS_INHERIT / RX=NO_INHERITANCE} →
//! SetEntriesInAclW（GRANT_ACCESS = 替换该 SID 现有 ACE 再追加，正是 icacls /grant
//! 语义）→ SetNamedSecurityInfoW(DACL_SECURITY_INFORMATION)。
//! 全程 unsafe；任何一步失败返回 false，不 panic。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, GENERIC_EXECUTE, GENERIC_READ, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
    EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID,
    TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};

/// 等价 `icacls "<path>" /grant "ALL APPLICATION PACKAGES:(OI)(CI)RX"`（inherit=true）
/// 或 `icacls "<path>" /grant "ALL APPLICATION PACKAGES:RX"`（inherit=false）。
pub fn grant_all_app_packages(path: &Path, inherit: bool) -> bool {
    unsafe {
        // UTF-16 安全：OsStr 原样转 wide，不丢码位；两条串各自 NUL 结尾
        let mut name16: Vec<u16> = path.as_os_str().encode_wide().collect();
        name16.push(0u16);
        let sid16: Vec<u16> = "S-1-15-2-1".encode_utf16().chain(Some(0u16)).collect();

        // 1) 取旧 DACL。返回的 SECURITY_DESCRIPTOR（DACL 指向其内部）由我们负责释放。
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        if GetNamedSecurityInfoW(
            PCWSTR::from_raw(name16.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl as *mut *mut ACL),
            None,
            &mut sd,
        )
        .0 != 0
        {
            return false;
        }

        // 2) S-1-15-2-1 = ALL APPLICATION PACKAGES（well-known 组 SID，以 SID 形式授权）
        let mut sid = PSID(std::ptr::null_mut());
        if ConvertStringSidToSidW(PCWSTR::from_raw(sid16.as_ptr()), &mut sid).is_err() {
            let _ = LocalFree(HLOCAL(sd.0));
            return false;
        }

        // 3) 授权项：RX=GENERIC_READ|GENERIC_EXECUTE；(OI)(CI)=SUB_CONTAINERS_AND_OBJECTS_INHERIT
        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_READ.0 | GENERIC_EXECUTE.0,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: if inherit {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                NO_INHERITANCE
            },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                ptstrName: PWSTR::from_raw(sid.0 as *mut u16),
            },
        };

        // 4) 新 DACL = 旧 DACL 合入该 SID 的 GRANT 项。SetEntriesInAclW 产出的新 ACL
        //    是独立分配，旧 SD 与 SID 在此之后即可释放。
        let mut new_acl: *mut ACL = std::ptr::null_mut();
        let rc = SetEntriesInAclW(
            Some(std::slice::from_ref(&ea)),
            Some(old_dacl as *const ACL),
            &mut new_acl,
        );
        let _ = LocalFree(HLOCAL(sd.0));
        let _ = LocalFree(HLOCAL(sid.0));
        if rc.0 != 0 {
            return false;
        }

        // 5) 只写回 DACL（不动 owner/SACL），随后释放新 ACL
        let rc = SetNamedSecurityInfoW(
            PCWSTR::from_raw(name16.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            PSID(std::ptr::null_mut()),
            PSID(std::ptr::null_mut()),
            Some(new_acl as *const ACL),
            None,
        );
        let _ = LocalFree(HLOCAL(new_acl as *mut core::ffi::c_void));
        rc.0 == 0
    }
}

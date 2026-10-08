use super::Entry;
use anyhow::Result;

#[cfg(target_os = "macos")]
fn center() -> Result<objc2::rc::Retained<objc2_user_notifications::UNUserNotificationCenter>> {
    use anyhow::ensure;
    use objc2_foundation::NSBundle;
    // UNUserNotificationCenter raises an ObjC exception for bare command-line
    // binaries. A debug cargo run still supports the in-app reminder list.
    ensure!(
        NSBundle::mainBundle().bundleIdentifier().is_some(),
        "系统通知需要通过 Starter.app 启动"
    );
    Ok(objc2_user_notifications::UNUserNotificationCenter::currentNotificationCenter())
}

#[cfg(target_os = "macos")]
pub fn authorize() -> Result<()> {
    use anyhow::{Context, ensure};
    use block2::RcBlock;
    use objc2::{rc::autoreleasepool, runtime::Bool};
    use objc2_foundation::NSError;
    use objc2_user_notifications::UNAuthorizationOptions;
    autoreleasepool(|_| {
        let center = center()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let callback = RcBlock::new(move |allowed: Bool, error: *mut NSError| {
            let result = if let Some(error) = unsafe { error.as_ref() } {
                Err(error.to_string())
            } else {
                Ok(allowed.as_bool())
            };
            let _ = sender.send(result);
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &callback,
        );
        let allowed = receiver
            .recv_timeout(std::time::Duration::from_secs(45))
            .context("通知权限尚未确认，请在系统设置中允许 Starter 通知")?
            .map_err(anyhow::Error::msg)?;
        ensure!(
            allowed,
            "系统通知未获允许，请在系统设置 → 通知中允许 Starter"
        );
        Ok(())
    })
}

#[cfg(target_os = "macos")]
pub fn deliver(entry: &Entry) -> Result<()> {
    use anyhow::{Context, ensure};
    use block2::RcBlock;
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSError, NSString};
    use objc2_user_notifications::{
        UNMutableNotificationContent, UNNotificationRequest, UNNotificationSound,
    };
    autoreleasepool(|_| {
        let center = center()?;
        let (settings_sender, settings_receiver) = std::sync::mpsc::channel();
        let settings_callback = RcBlock::new(
            move |settings: std::ptr::NonNull<objc2_user_notifications::UNNotificationSettings>| {
                let status = unsafe { settings.as_ref() }.authorizationStatus();
                let allowed = status != objc2_user_notifications::UNAuthorizationStatus::Denied
                    && status != objc2_user_notifications::UNAuthorizationStatus::NotDetermined;
                let _ = settings_sender.send(allowed);
            },
        );
        center.getNotificationSettingsWithCompletionHandler(&settings_callback);
        ensure!(
            settings_receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .context("系统通知状态读取超时")?,
            "请在系统设置 → 通知中允许 Starter 通知"
        );
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&entry.title));
        content.setBody(&NSString::from_str(&format!(
            "Starter 提醒 · {}",
            super::time::display(entry.due_at)
        )));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&entry.id),
            &content,
            None,
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        let callback = RcBlock::new(move |error: *mut NSError| {
            let result = unsafe { error.as_ref() }.map(|error| error.to_string());
            let _ = sender.send(result);
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&callback));
        if let Some(error) = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .context("系统通知发送超时")?
        {
            anyhow::bail!("系统通知发送失败：{error}");
        }
        Ok(())
    })
}

#[cfg(target_os = "windows")]
const APP_ID: &str = "dev.starter.launcher";
#[cfg(target_os = "windows")]
pub fn authorize() -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    // Register our own identity (also set on the MSI shortcut), so notifications
    // appear as Starter rather than borrowing PowerShell's identity.
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(format!("Software\\Classes\\AppUserModelId\\{APP_ID}"))?;
    key.set_value("DisplayName", &"Starter")?;
    key.set_value(
        "IconUri",
        &std::env::current_exe()?.to_string_lossy().as_ref(),
    )?;
    Ok(())
}
#[cfg(target_os = "windows")]
pub fn deliver(entry: &Entry) -> Result<()> {
    authorize()?;
    winrt_notification::Toast::new(APP_ID)
        .title(&entry.title)
        .text1(&format!(
            "Starter 提醒 · {}",
            super::time::display(entry.due_at)
        ))
        .sound(Some(winrt_notification::Sound::Default))
        .show()
        .map_err(|error| anyhow::anyhow!("系统通知发送失败：{error}"))?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn authorize() -> Result<()> {
    anyhow::bail!("当前平台暂不支持系统通知")
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn deliver(_: &Entry) -> Result<()> {
    authorize()
}

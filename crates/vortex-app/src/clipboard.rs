//! vortex-app/src/clipboard.rs
//! Cross-platform OS clipboard, native file picker, and folder launcher (macOS / Linux / Windows).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Reads plain text from the OS clipboard (native X11/Wayland/macOS/Windows via `arboard`, with CLI & GTK fallbacks).
pub fn read_clipboard_text() -> Option<String> {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(txt) = cb.get_text() {
            let s = txt.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        #[cfg(target_os = "linux")]
        {
            use arboard::{GetExtLinux, LinuxClipboardKind};
            if let Ok(txt) = cb.get().clipboard(LinuxClipboardKind::Primary).text() {
                let s = txt.trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("pbpaste").output().ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return if s.is_empty() { None } else { Some(s) };
        }
    }

    #[cfg(target_os = "linux")]
    {
        for (bin, args) in [
            ("wl-paste", vec!["--no-newline"]),
            ("xclip", vec!["-o", "-selection", "clipboard"]),
            ("xsel", vec!["-ob"]),
        ] {
            if let Ok(out) = Command::new(bin).args(&args).output() {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }

        // Fallback via system Python GTK3 (works cleanly on Ubuntu GNOME desktops without xclip/wl-paste)
        let py = "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk, Gdk; \
                  t = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text() or Gtk.Clipboard.get(Gdk.SELECTION_PRIMARY).wait_for_text() or ''; \
                  print(t.strip(), end='')";
        if let Ok(out) = Command::new("python3").args(["-c", py]).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Clipboard"])
            .output()
            .ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return if s.is_empty() { None } else { Some(s) };
        }
    }

    None
}

/// Opens a native OS file picker dialog to choose a `.torrent` file.
pub fn pick_torrent_file() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"POSIX path of (choose file with prompt "Select a .torrent file" of type {"torrent"})"#;
        let out = Command::new("osascript").args(["-e", script]).output().ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(out) = Command::new("zenity")
            .args(["--file-selection", "--title=Select .torrent file", "--file-filter=*.torrent"])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
        if let Ok(out) = Command::new("kdialog")
            .args(["--getopenfilename", ".", "*.torrent"])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let ps = "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.OpenFileDialog; $f.Filter = 'Torrent files (*.torrent)|*.torrent'; if ($f.ShowDialog() -eq 'OK') { $f.FileName }";
        let out = Command::new("powershell").args(["-NoProfile", "-Command", ps]).output().ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }

    None
}

/// Opens a native OS folder picker dialog to choose where files and videos are downloaded.
pub fn pick_download_folder(current: Option<&Path>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let _ = current;
        let script = r#"POSIX path of (choose folder with prompt "Select Vortex Download Folder")"#;
        let out = Command::new("osascript").args(["-e", script]).output().ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let mut z_args = vec![
            "--file-selection".to_string(),
            "--directory".to_string(),
            "--title=Select Download Folder for Files & Videos".to_string(),
        ];
        if let Some(dir) = current {
            let mut s = dir.to_string_lossy().to_string();
            if !s.ends_with('/') {
                s.push('/');
            }
            z_args.push(format!("--filename={}", s));
        }
        if let Ok(out) = Command::new("zenity").args(&z_args).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }

        let start_dir = current
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        if let Ok(out) = Command::new("kdialog")
            .args(["--getexistingdirectory", &start_dir])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }

        let py = "import gi, sys; gi.require_version('Gtk','3.0'); from gi.repository import Gtk; \
                  d = Gtk.FileChooserDialog(title='Select Download Folder', action=Gtk.FileChooserAction.SELECT_FOLDER); \
                  d.add_buttons(Gtk.STOCK_CANCEL, Gtk.ResponseType.CANCEL, Gtk.STOCK_OPEN, Gtk.ResponseType.OK); \
                  res = d.run(); p = d.get_filename() if res == Gtk.ResponseType.OK else ''; d.destroy(); \
                  print(p or '', end='')";
        if let Ok(out) = Command::new("python3").args(["-c", py]).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let _ = current;
        let ps = "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = 'Select Vortex Download Folder'; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }";
        let out = Command::new("powershell").args(["-NoProfile", "-Command", ps]).output().ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }

    None
}

/// Opens a file or folder in the native OS file manager.
pub fn open_in_file_manager(path: &Path) {
    let dir = if path.is_file() {
        path.parent().unwrap_or(path)
    } else if path.exists() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let _ = std::fs::create_dir_all(dir);

    #[cfg(target_os = "macos")]
    {
        if path.is_file() {
            let _ = Command::new("open").arg("-R").arg(path).spawn();
        } else {
            let _ = Command::new("open").arg(dir).spawn();
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(dir).spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("explorer").arg(dir).spawn();
    }
}

/// Launches a downloaded video or media file in the OS default player.
pub fn open_media_file(path: &Path) {
    if !path.exists() {
        open_in_file_manager(path);
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(path).spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn();
    }
}

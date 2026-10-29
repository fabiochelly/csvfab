-- csvfab.app: the macOS application. Opening a CSV from the Finder sends an Apple
-- Event that only an application bundle receives, so this applet is the bundle:
-- it turns "open these files" into a csvfab command line and quits. The program
-- itself sits in Contents/Resources/csvfab; started while a csvfab window is open,
-- it hands the files to that window and stops.
-- Built by macos/build-app.sh (osacompile), which also declares the document
-- types in Info.plist so csvfab shows in "Open with" and can be made the default.

on launcher()
    return POSIX path of (path to me) & "Contents/Resources/csvfab"
end launcher

on launch_with(theFiles)
    set cmd to quoted form of launcher()
    repeat with f in theFiles
        set cmd to cmd & " " & quoted form of (POSIX path of f)
    end repeat
    -- Detached and silent, or the applet would wait for the window to close.
    do shell script cmd & " >/dev/null 2>&1 &"
end launch_with

on run
    launch_with({})
end run

on open theFiles
    launch_with(theFiles)
end open

on reopen
    launch_with({})
end reopen

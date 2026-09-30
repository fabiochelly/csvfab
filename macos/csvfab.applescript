-- csvfab.app: the macOS application. Opening a CSV from the Finder sends an Apple
-- Event that only an application bundle receives, so this applet is the bundle:
-- it turns "open these files" into a csvfab command line and quits. The app
-- itself (csvfab.py, server.py, the page) sits in Contents/Resources/csvfab.
-- Built by macos/build-app.sh (osacompile), which also declares the document
-- types in Info.plist so csvfab shows in "Open with" and can be made the default.

on launcher()
    return POSIX path of (path to me) & "Contents/Resources/csvfab/csvfab.py"
end launcher

-- Homebrew's Python first; /usr/bin/python3 is a stub that offers the Command
-- Line Tools when they are missing, which is at least a way forward.
on python()
    repeat with p in {"/opt/homebrew/bin/python3", "/usr/local/bin/python3", "/usr/bin/python3"}
        try
            do shell script "test -x " & quoted form of p
            return p
        end try
    end repeat
    return "python3"
end python

on launch_with(theFiles)
    set cmd to quoted form of python() & " " & quoted form of launcher()
    repeat with f in theFiles
        set cmd to cmd & " " & quoted form of (POSIX path of f)
    end repeat
    -- Detached and silent, or the applet would wait for the server the launcher starts.
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

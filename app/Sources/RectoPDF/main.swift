import AppKit

// A QA run is decided before AppKit has done anything, so there's no moment when it could take the screen.
let launchArguments = Array(CommandLine.arguments.dropFirst())
// AppKit adds Dictation and Emoji to any menu called Edit. A reader has nothing to type into, so they'd do nothing, and they cost about 0.35 MiB, measured side by side. Said before there's an application at all: a menu bar built from a model that arrives before `applicationDidFinishLaunching` got Emoji anyway, which a restore test caught after a relaunch.
UserDefaults.standard.register(defaults: ["NSDisabledDictationMenuItem": true, "NSDisabledCharacterPaletteMenuItem": true])
let application = NSApplication.shared
application.setActivationPolicy(launchIsQuiet(args: launchArguments) ? .prohibited : .regular)
let appDelegate = MainActor.assumeIsolated { AppDelegate(arguments: launchArguments) }
application.delegate = appDelegate
application.run()

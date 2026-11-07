import AppKit

// A QA run is decided before AppKit has done anything, so there's no moment when it could take the screen.
let launchArguments = Array(CommandLine.arguments.dropFirst())
let application = NSApplication.shared
application.setActivationPolicy(launchArguments.contains("--qa") ? .prohibited : .regular)
let appDelegate = MainActor.assumeIsolated { AppDelegate(arguments: launchArguments) }
application.delegate = appDelegate
application.run()

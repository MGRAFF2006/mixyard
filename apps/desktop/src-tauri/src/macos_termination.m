#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#include <stdio.h>

// One application/delegate for the process lifetime. Preserve Tao's delegate
// identity and every existing method; never replace it with a partial proxy.
static void (*request_quit)(void) = NULL;

static NSApplicationTerminateReply should_terminate(id delegate, SEL selector,
                                                    NSApplication *sender) {
  (void)delegate;
  (void)selector;
  (void)sender;
  // Returning Cancel immediately keeps AppKit responsive while the existing
  // webview polls its UI action and asks the aggregate retained-draft guard.
  // Accepted quit_ui uses Tauri's explicit event-loop exit, not terminate:.
  if (request_quit != NULL) {
    request_quit();
  }
  return NSTerminateCancel;
}

const char *mixyard_install_termination_guard(void (*callback)(void)) {
  if (![NSThread isMainThread]) {
    return "AppKit termination guard must be installed on the main thread";
  }
  if (callback == NULL || request_quit != NULL) {
    return "AppKit termination guard callback is missing or already installed";
  }
  id delegate = [NSApp delegate];
  if (delegate == nil) {
    return "AppKit application delegate is unavailable";
  }
  Class delegate_class = object_getClass(delegate);
  SEL selector = @selector(applicationShouldTerminate:);
  // A dependency that adds its own interception needs deliberate integration,
  // not an invisible override of its termination policy.
  if (class_getInstanceMethod(delegate_class, selector) != NULL) {
    return "Application delegate already handles applicationShouldTerminate:";
  }
  char encoding[32];
  snprintf(encoding, sizeof(encoding), "%s@:@", @encode(NSApplicationTerminateReply));
  if (!class_addMethod(delegate_class, selector, (IMP)should_terminate, encoding)) {
    return "Cannot install preventable AppKit termination callback";
  }
  request_quit = callback;
  // Re-register the same retained instance after adding the optional method;
  // AppKit may cache which delegate selectors are implemented. No event loop
  // runs while it is temporarily unbound, and Tao still owns the same object.
  [NSApp setDelegate:nil];
  [NSApp setDelegate:delegate];
  if ([NSApp delegate] != delegate) {
    return "AppKit did not retain the existing application delegate registration";
  }
  return NULL;
}

// Actual AppKit boundary probe; no accessibility/AppleEvents/input injection.
#import <AppKit/AppKit.h>
#import <dispatch/dispatch.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

extern const char *mixyard_install_termination_guard(void (*callback)(void));

static NSWindow *probe_window;
static NSTextField *marker;
static NSMenu *application_menu;
static id retained_delegate;
static BOOL guarded;
static NSUInteger requests;
static BOOL approved;
static NSInteger initial_window;

static void fail(const char *message) {
  fprintf(stderr, "AppKit termination probe failed: %s\n", message);
  exit(2);
}

static void request_next(void);

static void request_quit(void) {
  requests++;
  // Model only the host decision boundary here. Actual retained Vue child-draft
  // aggregation is exercised independently by the shared browser tests.
  dispatch_async(dispatch_get_main_queue(), ^{
    if (![probe_window isVisible] || [probe_window windowNumber] != initial_window ||
        ![[marker stringValue] isEqualToString:@"native-window-retention-sentinel"]) {
      fail("cancel did not retain the same visible native window/content");
    }
    if (requests <= 2) {
      printf("{\"event\":\"cancelled\",\"origin\":\"%s\",\"pid\":%d,\"window\":%ld}\n",
             requests == 1 ? "menu-terminate" : "dock-equivalent-terminate", getpid(), (long)initial_window);
      fflush(stdout);
      dispatch_async(dispatch_get_main_queue(), ^{ request_next(); });
    } else if (requests == 3) {
      approved = YES;
      // Match Tao's explicit approved event-loop exit: stop, then wake the
      // event loop. Do not ask AppKit to terminate (and reenter the guard).
      [NSApp stop:nil];
      NSEvent *wake = [NSEvent otherEventWithType:NSEventTypeApplicationDefined
        location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil
        subtype:0 data1:0 data2:0];
      [NSApp postEvent:wake atStart:YES];
    } else {
      fail("termination callback dispatched repeatedly");
    }
  });
}

static void request_next(void) {
  if (requests == 1) {
    // Dock's Quit reaches this same NSApplication terminate: boundary. This
    // calls it directly; it does not claim physical Dock hit-testing.
    [NSApp terminate:nil];
  } else {
    // Actual NSMenu dispatch, with the predefined Quit selector/key equivalent
    // used by locked Muda. No synthetic keyboard event or menu-only workaround.
    [application_menu performActionForItemAtIndex:0];
  }
}

@interface TerminationProbeDelegate : NSObject <NSApplicationDelegate>
@end
@implementation TerminationProbeDelegate
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
  (void)notification;
  [probe_window makeKeyAndOrderFront:nil];
  initial_window = [probe_window windowNumber];
  dispatch_async(dispatch_get_main_queue(), ^{ request_next(); });
}
- (void)applicationWillTerminate:(NSNotification *)notification {
  (void)notification;
  printf("{\"event\":\"native-will-terminate\",\"guarded\":%s,\"pid\":%d}\n", guarded ? "true" : "false", getpid());
  fflush(stdout);
  if (guarded) fail("AppKit bypassed the preventable delegate callback");
}
@end

int main(int argc, const char *argv[]) {
  @autoreleasepool {
    if (getenv("MIXYARD_MACOS_TERMINATION_PROBE") == NULL) {
      fail("requires the owned hosted runner probe wrapper");
    }
    guarded = !(argc == 2 && strcmp(argv[1], "--without-guard") == 0);
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
    retained_delegate = [TerminationProbeDelegate new];
    [NSApp setDelegate:retained_delegate];
    probe_window = [[NSWindow alloc] initWithContentRect:NSMakeRect(50, 50, 460, 240)
      styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
      backing:NSBackingStoreBuffered defer:NO];
    [probe_window setTitle:@"Mixyard owned AppKit termination probe"];
    marker = [NSTextField labelWithString:@"native-window-retention-sentinel"];
    [marker setFrame:NSMakeRect(20, 80, 410, 40)];
    [[probe_window contentView] addSubview:marker];
    NSMenu *menu = [NSMenu new];
    NSMenuItem *app_item = [NSMenuItem new];
    application_menu = [[NSMenu alloc] initWithTitle:@"Mixyard native probe"];
    NSMenuItem *quit = [[NSMenuItem alloc] initWithTitle:@"Quit UI" action:@selector(terminate:) keyEquivalent:@"q"];
    [quit setTarget:NSApp];
    [application_menu addItem:quit];
    [app_item setSubmenu:application_menu];
    [menu addItem:app_item];
    [NSApp setMainMenu:menu];
    if (guarded) {
      const char *error = mixyard_install_termination_guard(request_quit);
      if (error != NULL) fail(error);
      if ([NSApp delegate] != retained_delegate) fail("guard replaced the application delegate");
      if (mixyard_install_termination_guard(request_quit) == NULL) fail("duplicate guard silently replaced native policy");
    }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
      fail("native event-loop deadline exceeded");
    });
    [NSApp run];
    if (!guarded || !approved || requests != 3) fail("event loop exited before explicit approval");
    printf("{\"event\":\"approved-ui-exit\",\"pid\":%d,\"window\":%ld,\"requests\":%lu}\n",
           getpid(), (long)initial_window, (unsigned long)requests);
    return 0;
  }
}

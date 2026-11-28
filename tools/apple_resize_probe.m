// Build this test-only dylib with clang -dynamiclib -fobjc-arc -framework AppKit
// -framework QuartzCore -framework Metal tools/apple_resize_probe.m -o /tmp/octara-resize.dylib.
// Run it with DYLD_INSERT_LIBRARIES and OCTARA_RESIZE_RATE=15, 30, or 60 in the window example.
#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#import <objc/runtime.h>
#include <stdatomic.h>

static id<CAMetalDrawable> (*originalNextDrawable)(CAMetalLayer *, SEL);
static atomic_ulong sequence;

static id<CAMetalDrawable> nextDrawable(CAMetalLayer *layer, SEL selector) {
    id<CAMetalDrawable> drawable = originalNextDrawable(layer, selector);
    if (drawable == nil) return nil;
    unsigned long ordinal = atomic_fetch_add_explicit(&sequence, 1, memory_order_relaxed);
    fprintf(stdout, "resize-probe,acquire,%lu,%.9f,%lu,%lu\n", ordinal, CACurrentMediaTime(),
            (unsigned long)drawable.texture.width, (unsigned long)drawable.texture.height);
    [drawable addPresentedHandler:^(id<MTLDrawable> presented) {
        fprintf(stdout, "resize-probe,present,%lu,%.9f\n", ordinal, presented.presentedTime);
    }];
    return drawable;
}

static void driveWindow(NSWindow *window, NSInteger rate) {
    NSScreen *screen = NSScreen.screens.firstObject;
    NSRect frame = NSMakeRect(NSMinX(screen.visibleFrame) + 80, NSMinY(screen.visibleFrame) + 60, 720, 610);
    [window setFrame:frame display:YES animate:NO];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 600 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        __block NSInteger index = 0;
        NSTimer *timer = [NSTimer timerWithTimeInterval:1.0 / rate repeats:YES block:^(NSTimer *tick) {
            NSRect next = frame;
            next.size.width += 2 * (index % 51);
            next.size.height += index % 37;
            [window setFrame:next display:YES animate:NO];
            NSView *content = window.contentView;
            NSSize size = [content convertSizeToBacking:content.bounds.size];
            fprintf(stdout, "resize-probe,resize,%ld,%.9f,%.0f,%.0f\n",
                    (long)index, CACurrentMediaTime(), size.width, size.height);
            if (++index == 2 * rate) {
                [tick invalidate];
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 750 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                    fflush(stdout);
                    [NSApp terminate:nil];
                });
            }
        }];
        [NSRunLoop.mainRunLoop addTimer:timer forMode:NSRunLoopCommonModes];
    });
}

static void awaitWindow(NSInteger rate) {
    for (NSWindow *window in NSApp.windows) {
        if (window.isVisible && [NSStringFromClass(window.class) hasPrefix:@"Octara"]) {
            driveWindow(window, rate);
            return;
        }
    }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 25 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        awaitWindow(rate);
    });
}

__attribute__((constructor)) static void installProbe(void) {
    const char *value = getenv("OCTARA_RESIZE_RATE");
    if (value == NULL) return;
    NSInteger rate = strtol(value, NULL, 10);
    if (rate != 15 && rate != 30 && rate != 60) return;
    Method method = class_getInstanceMethod(CAMetalLayer.class, @selector(nextDrawable));
    originalNextDrawable = (void *)method_setImplementation(method, (IMP)nextDrawable);
    setvbuf(stdout, NULL, _IOLBF, 0);
    dispatch_async(dispatch_get_main_queue(), ^{
        awaitWindow(rate);
    });
}

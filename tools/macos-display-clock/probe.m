// AppKit/Metal display-clock probe. Build and run as described in README.md.
#import <AppKit/AppKit.h>
#import <CoreVideo/CoreVideo.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>

static const NSTimeInterval stageDuration = 4.0;

static CGDirectDisplayID displayID(NSScreen *screen) {
    return [screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
}

static void fail(NSString *message) {
    fprintf(stderr, "%s\n", message.UTF8String);
    exit(EXIT_FAILURE);
}

@interface Probe : NSObject <NSWindowDelegate, CAMetalDisplayLinkDelegate>
@property (nonatomic, strong) NSWindow *window;
@property (nonatomic, strong) CAMetalLayer *layer;
@property (nonatomic, strong) id<MTLCommandQueue> queue;
@property (nonatomic, strong) CADisplayLink *caLink;
@property (nonatomic, strong) CAMetalDisplayLink *metalLink;
@property (nonatomic) CVDisplayLinkRef videoLink;
@property (nonatomic, strong) NSScreen *internal;
@property (nonatomic, strong) NSScreen *external;
@property (nonatomic, copy) NSString *driver;
@property (nonatomic, copy) NSString *route;
@property (nonatomic, copy) NSString *stage;
@property (nonatomic) CFTimeInterval stageStart;
@property (nonatomic) CFTimeInterval videoBindTime;
@property (nonatomic) NSUInteger sequence;
- (void)run;
- (void)recordTick:(CFTimeInterval)callback
            target:(CFTimeInterval)target
          interval:(CFTimeInterval)interval
          drawable:(id<CAMetalDrawable>)drawable;
@end

static CVReturn videoTick(CVDisplayLinkRef link, const CVTimeStamp *now,
                          const CVTimeStamp *output, CVOptionFlags inFlags,
                          CVOptionFlags *outFlags, void *context) {
    CFTimeInterval callback = CACurrentMediaTime();
    CFTimeInterval target = output->hostTime / CVGetHostClockFrequency();
    CFTimeInterval interval = (double)output->videoRefreshPeriod / output->videoTimeScale;
    Probe *probe = (__bridge Probe *)context;
    dispatch_async(dispatch_get_main_queue(), ^{
        if (probe.videoLink == link && callback >= probe.videoBindTime) {
            [probe recordTick:callback target:target interval:interval drawable:nil];
        }
    });
    return kCVReturnSuccess;
}

@implementation Probe

- (CAFrameRateRange)rangeForScreen:(NSScreen *)screen {
    float maximum = (float)MAX(screen.maximumFramesPerSecond, 1);
    if ([_driver hasSuffix:@"-default"]) {
        return CAFrameRateRangeDefault;
    }
    if ([_driver hasSuffix:@"-low"]) {
        return CAFrameRateRangeMake(MIN(24, maximum), MIN(40, maximum), MIN(30, maximum));
    }
    float minimum = [_driver hasSuffix:@"-fixed"] ? maximum : MIN(80, maximum);
    return CAFrameRateRangeMake(minimum, maximum, maximum);
}

- (void)stopVideoLink {
    if (_videoLink) {
        CVDisplayLinkStop(_videoLink);
        CVDisplayLinkRelease(_videoLink);
        _videoLink = NULL;
    }
}

- (void)bindScreen {
    NSScreen *screen = _window.screen;
    _layer.contentsScale = _window.backingScaleFactor;
    _layer.drawableSize = [_window.contentView convertRectToBacking:_window.contentView.bounds].size;
    if (_metalLink) {
        _metalLink.preferredFrameRateRange = [self rangeForScreen:screen];
    } else if ([_driver hasPrefix:@"view"]) {
        _caLink.preferredFrameRateRange = [self rangeForScreen:screen];
    } else if ([_driver hasPrefix:@"screen"]) {
        [_caLink invalidate];
        _caLink = [screen displayLinkWithTarget:self selector:@selector(caTick:)];
        _caLink.preferredFrameRateRange = [self rangeForScreen:screen];
        [_caLink addToRunLoop:NSRunLoop.mainRunLoop forMode:NSRunLoopCommonModes];
    } else if ([_driver isEqualToString:@"cv"]) {
        [self stopVideoLink];
        _videoBindTime = CACurrentMediaTime();
        if (CVDisplayLinkCreateWithCGDisplay(displayID(screen), &_videoLink) != kCVReturnSuccess ||
            CVDisplayLinkSetOutputCallback(_videoLink, videoTick, (__bridge void *)self) != kCVReturnSuccess ||
            CVDisplayLinkStart(_videoLink) != kCVReturnSuccess) {
            fail(@"Could not start CVDisplayLink for the window's display");
        }
    }
}

- (void)windowDidChangeScreen:(NSNotification *)notification {
    if (_window.screen) {
        fprintf(stderr, "screen notification at %.9f, stage=%s, display=%u\n",
                CACurrentMediaTime(), _stage.UTF8String, displayID(_window.screen));
        [self bindScreen];
    }
}

- (void)caTick:(CADisplayLink *)link {
    [self recordTick:CACurrentMediaTime() target:link.targetTimestamp
            interval:link.targetTimestamp - link.timestamp drawable:nil];
}

- (void)metalDisplayLink:(CAMetalDisplayLink *)link needsUpdate:(CAMetalDisplayLinkUpdate *)update {
    [self recordTick:CACurrentMediaTime() target:update.targetPresentationTimestamp
            interval:update.targetPresentationTimestamp - update.targetTimestamp drawable:update.drawable];
}

- (void)recordTick:(CFTimeInterval)callback target:(CFTimeInterval)target
          interval:(CFTimeInterval)interval drawable:(id<CAMetalDrawable>)drawable {
    CFTimeInterval main = CACurrentMediaTime();
    if (!_stage || callback < _stageStart) {
        return;
    }
    // Keep delayed Core Video ticks in the callback log without rendering a backlog.
    BOOL stale = _videoLink && interval > 0 && main - callback > interval;
    BOOL metal = _metalLink != nil;
    if (!drawable && !metal && !stale) {
        drawable = [_layer nextDrawable];
    }
    CFTimeInterval acquired = CACurrentMediaTime();
    NSString *stage = _stage;
    NSUInteger serial = ++_sequence;
    CGDirectDisplayID display = displayID(_window.screen);
    printf("tick,%s,%u,%lu,%.9f,%.9f,%.9f,%.9f,%.9f,%s,\n",
           stage.UTF8String, display, (unsigned long)serial, callback, main, target, interval,
           acquired, stale ? "stale" : (drawable ? "drawable" : "missing"));
    if (!drawable) {
        return;
    }

    MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
    MTLRenderPassColorAttachmentDescriptor *color = pass.colorAttachments[0];
    color.texture = drawable.texture;
    color.loadAction = MTLLoadActionClear;
    color.storeAction = MTLStoreActionStore;
    color.clearColor = MTLClearColorMake(0.1, 0.25 + (serial % 23) * 0.02, 0.52, 1.0);
    id<MTLCommandBuffer> buffer = [_queue commandBuffer];
    id<MTLRenderCommandEncoder> encoder = [buffer renderCommandEncoderWithDescriptor:pass];
    if (!encoder) {
        fail(@"Could not encode the Metal clear pass");
    }
    [encoder endEncoding];
    [drawable addPresentedHandler:^(id<MTLDrawable> presented) {
        printf("present,%s,%u,%lu,,,,,,,%.9f\n",
               stage.UTF8String, display, (unsigned long)serial, presented.presentedTime);
    }];
    if (metal) {
        // CAMetalDisplayLink gives us a drawable and a deadline to call present.
        [buffer commit];
        [drawable present];
    } else {
        [buffer presentDrawable:drawable];
        [buffer commit];
    }
}

- (void)beginStage:(NSString *)stage onScreen:(NSScreen *)screen {
    _stage = stage;
    _stageStart = CACurrentMediaTime();
    NSRect bounds = screen.visibleFrame;
    NSRect frame = _window.frame;
    frame.origin = NSMakePoint(NSMidX(bounds) - frame.size.width / 2, NSMidY(bounds) - frame.size.height / 2);
    [_window setFrame:frame display:YES];
    if (displayID(_window.screen) != displayID(screen)) {
        fail(@"AppKit did not move the probe window to the requested display");
    }
    fprintf(stderr, "stage=%s start=%.9f display=%u AppKit=%ld Hz\n",
            stage.UTF8String, _stageStart, displayID(screen), (long)screen.maximumFramesPerSecond);
    printf("stage,%s,%u,,%.9f,,,,,,\n", stage.UTF8String, displayID(screen), _stageStart);
}

- (void)afterStage {
    if ([_stage isEqualToString:@"internal"]) {
        [self beginStage:@"moved-external" onScreen:_external];
    } else if ([_stage isEqualToString:@"moved-external"]) {
        [self beginStage:@"returned-internal" onScreen:_internal];
    } else {
        [_caLink invalidate];
        [_metalLink invalidate];
        [self stopVideoLink];
        // Wait for the last drawable acknowledgements before closing the window.
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            fflush(stdout);
            [NSApp stop:nil];
            [NSApp postEvent:[NSEvent otherEventWithType:NSEventTypeApplicationDefined
                  location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil
                  subtype:0 data1:0 data2:0] atStart:NO];
        });
        return;
    }
    [self scheduleStageEnd];
}

- (void)scheduleStageEnd {
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(stageDuration * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{ [self afterStage]; });
}

- (void)run {
    for (NSScreen *screen in NSScreen.screens) {
        CGDirectDisplayID display = displayID(screen);
        CGDisplayModeRef mode = CGDisplayCopyDisplayMode(display);
        double modeRate = mode ? CGDisplayModeGetRefreshRate(mode) : 0;
        if (mode) {
            CGDisplayModeRelease(mode);
        }
        fprintf(stderr, "display=%u name=%s builtIn=%d AppKit=%ld Hz mode=%.3f Hz\n",
                display, screen.localizedName.UTF8String, CGDisplayIsBuiltin(display) != 0,
                (long)screen.maximumFramesPerSecond, modeRate);
        if (CGDisplayIsBuiltin(display)) {
            _internal = screen;
        } else if (!_external || screen.maximumFramesPerSecond > _external.maximumFramesPerSecond) {
            _external = screen;
        }
    }
    if (!_internal || !_external) {
        fail(@"This probe requires a built-in screen and an external screen");
    }
    BOOL move = [_route isEqualToString:@"move"];
    NSScreen *first = move ? _internal : _external;
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    _queue = [device newCommandQueue];
    if (!_queue) {
        fail(@"Could not create a Metal command queue");
    }
    _window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 480, 320)
                                         styleMask:NSWindowStyleMaskTitled
                                           backing:NSBackingStoreBuffered defer:NO screen:first];
    _window.title = [NSString stringWithFormat:@"Octara display clock — %@", _driver];
    _window.level = NSFloatingWindowLevel;
    _layer = [CAMetalLayer layer];
    _layer.device = device;
    _layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
    _window.contentView.layer = _layer;
    _window.contentView.wantsLayer = YES;
    if ([_driver hasPrefix:@"view"]) {
        // Match Octara's original creation order, before showing the window.
        _caLink = [_window.contentView displayLinkWithTarget:self selector:@selector(caTick:)];
        [_caLink addToRunLoop:NSRunLoop.mainRunLoop forMode:NSRunLoopCommonModes];
    } else if ([_driver hasPrefix:@"metal"]) {
        _metalLink = [[CAMetalDisplayLink alloc] initWithMetalLayer:_layer];
        _metalLink.delegate = self;
        _metalLink.preferredFrameLatency = 1;
        [_metalLink addToRunLoop:NSRunLoop.mainRunLoop forMode:NSRunLoopCommonModes];
    }
    puts("kind,stage,display,sequence,callback,main,target,interval_or_lead,acquired,acquisition,presented");
    [self beginStage:move ? @"internal" : @"external" onScreen:first];
    _window.delegate = self;
    [_window makeKeyAndOrderFront:nil];
    [NSApp activate];
    [self bindScreen];
    [self scheduleStageEnd];
}

@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSArray<NSString *> *drivers = @[@"view", @"view-default", @"screen", @"screen-default",
                                         @"screen-low", @"metal-wide", @"metal-fixed", @"metal-default", @"cv"];
        if (argc != 3) {
            fail(@"Usage: probe DRIVER external|move (see README.md for drivers)");
        }
        Probe *probe = [Probe new];
        probe.driver = @(argv[1]);
        probe.route = @(argv[2]);
        if (![drivers containsObject:probe.driver] ||
            ![@[@"external", @"move"] containsObject:probe.route]) {
            fail(@"Invalid driver or route");
        }
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        dispatch_async(dispatch_get_main_queue(), ^{ [probe run]; });
        [NSApp run];
    }
}

// iOS entry: hand over to Rust (src/ios_main.rs). winit starts
// UIApplicationMain itself.
#import <AVFoundation/AVFoundation.h>
#import <CoreVideo/CoreVideo.h>
#import <UIKit/UIKit.h>
#import <mach/mach.h>
#import <Metal/Metal.h>
#include <stdlib.h>

uint64_t l4d2r_memory_bytes(void) {
    task_vm_info_data_t info = {0};
    mach_msg_type_number_t count = TASK_VM_INFO_COUNT;
    if (task_info(mach_task_self(), TASK_VM_INFO, (task_info_t)&info, &count) != KERN_SUCCESS) return 0;
    return info.phys_footprint;
}

// Bytes Metal has allocated for this process (textures, buffers): the GPU
// share of the footprint, logged beside it to tell GPU and CPU growth apart.
uint64_t l4d2r_metal_bytes(void) {
    static id<MTLDevice> device;
    if (!device) device = MTLCreateSystemDefaultDevice();
    return device ? (uint64_t)device.currentAllocatedSize : 0;
}

extern void main_rs(void);

// Used only when Rust cannot start the game. Winit owns UIApplicationMain
// on successful launches; this path must never run after its event loop.
static NSString *startupMessage;
@interface L4D2StartupDelegate : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation L4D2StartupDelegate
- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)options {
    self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];
    UIViewController *controller = [UIViewController new];
    controller.view.backgroundColor = [UIColor colorWithWhite:0.06 alpha:1];
    UITextView *text = [UITextView new];
    text.translatesAutoresizingMaskIntoConstraints = NO;
    text.backgroundColor = UIColor.clearColor;
    text.textColor = UIColor.whiteColor;
    text.font = [UIFont systemFontOfSize:18];
    text.editable = NO;
    text.text = startupMessage;
    [controller.view addSubview:text];
    UILayoutGuide *safe = controller.view.safeAreaLayoutGuide;
    [NSLayoutConstraint activateConstraints:@[
        [text.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor constant:24],
        [text.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor constant:-24],
        [text.topAnchor constraintEqualToAnchor:safe.topAnchor constant:16],
        [text.bottomAnchor constraintEqualToAnchor:safe.bottomAnchor constant:-16]
    ]];
    self.window.rootViewController = controller;
    [self.window makeKeyAndVisible];
    return YES;
}
@end

void l4d2r_startup_error(const char *message) {
    @autoreleasepool {
        startupMessage = [NSString stringWithUTF8String:message];
        UIApplicationMain(0, NULL, nil, NSStringFromClass(L4D2StartupDelegate.class));
    }
}

int main(int argc, char *argv[]) {
    setenv("MIMALLOC_PURGE_DELAY", "0", 1);
    setenv("MIMALLOC_ARENA_EAGER_COMMIT", "0", 1);
    (void)argc;
    (void)argv;
    main_rs();
    return 0;
}

// Menu background video (src/menu_video.rs): AVAssetReader decodes the
// H.264 copy of the retail Bink to width x height RGBA frames.
@interface L4D2RVideo : NSObject
@property(nonatomic, strong) AVAssetReader *reader;
@property(nonatomic, strong) AVAssetReaderTrackOutput *output;
@property(nonatomic) int width;
@property(nonatomic) int height;
@end
@implementation L4D2RVideo
@end

void *l4d2r_video_open(const char *path, int width, int height) {
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
        AVURLAsset *asset = [AVURLAsset URLAssetWithURL:url options:nil];
        NSArray<AVAssetTrack *> *tracks = [asset tracksWithMediaType:AVMediaTypeVideo];
        if (tracks.count == 0) return NULL;
        NSError *error = nil;
        AVAssetReader *reader = [AVAssetReader assetReaderWithAsset:asset error:&error];
        if (!reader) return NULL;
        NSDictionary *settings = @{
            (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA),
            (id)kCVPixelBufferWidthKey : @(width),
            (id)kCVPixelBufferHeightKey : @(height),
        };
        AVAssetReaderTrackOutput *output =
            [AVAssetReaderTrackOutput assetReaderTrackOutputWithTrack:tracks[0] outputSettings:settings];
        output.alwaysCopiesSampleData = NO;
        if (![reader canAddOutput:output]) return NULL;
        [reader addOutput:output];
        if (![reader startReading]) return NULL;
        L4D2RVideo *video = [L4D2RVideo new];
        video.reader = reader;
        video.output = output;
        video.width = width;
        video.height = height;
        return (__bridge_retained void *)video;
    }
}

// Fills `rgba` (width * height * 4) with the next frame. 0 at the end.
int l4d2r_video_frame(void *handle, uint8_t *rgba) {
    if (!handle) return 0;
    @autoreleasepool {
        L4D2RVideo *video = (__bridge L4D2RVideo *)handle;
        CMSampleBufferRef sample = [video.output copyNextSampleBuffer];
        if (!sample) return 0;
        CVImageBufferRef image = CMSampleBufferGetImageBuffer(sample);
        int ok = 0;
        if (image) {
            CVPixelBufferLockBaseAddress(image, kCVPixelBufferLock_ReadOnly);
            const uint8_t *base = CVPixelBufferGetBaseAddress(image);
            size_t stride = CVPixelBufferGetBytesPerRow(image);
            size_t w = CVPixelBufferGetWidth(image);
            size_t h = CVPixelBufferGetHeight(image);
            if (w > (size_t)video.width) w = video.width;
            if (h > (size_t)video.height) h = video.height;
            for (size_t y = 0; y < h; y++) {
                const uint8_t *src = base + y * stride;
                uint8_t *dst = rgba + y * video.width * 4;
                for (size_t x = 0; x < w; x++) {
                    dst[0] = src[2];
                    dst[1] = src[1];
                    dst[2] = src[0];
                    dst[3] = 255;
                    src += 4;
                    dst += 4;
                }
            }
            CVPixelBufferUnlockBaseAddress(image, kCVPixelBufferLock_ReadOnly);
            ok = 1;
        }
        CFRelease(sample);
        return ok;
    }
}

void l4d2r_video_close(void *handle) {
    if (!handle) return;
    L4D2RVideo *video = (__bridge_transfer L4D2RVideo *)handle;
    [video.reader cancelReading];
}

// Safe-area insets of the key window in points (notch, Dynamic Island,
// home indicator), for src/game_safe_area.rs.
void l4d2r_safe_area(float *left, float *top, float *right, float *bottom) {
    *left = *top = *right = *bottom = 0;
    __block UIEdgeInsets insets = UIEdgeInsetsZero;
    void (^read)(void) = ^{
        for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
            if (![scene isKindOfClass:UIWindowScene.class]) continue;
            for (UIWindow *window in ((UIWindowScene *)scene).windows) {
                if (window.isKeyWindow || insets.left + insets.right + insets.top + insets.bottom == 0) {
                    insets = window.safeAreaInsets;
                }
            }
        }
    };
    // Bevy's main-thread systems call this; elsewhere UIKit is off limits
    // (a dispatch_sync to the busy main thread would deadlock).
    if (!NSThread.isMainThread) return;
    read();
    *left = insets.left;
    *top = insets.top;
    *right = insets.right;
    *bottom = insets.bottom;
}

// Render resolution (src/ui_fit.rs apply_render_scale): the game view's
// contentScaleFactor, clamped to the screen's native scale. Writes the
// view's size in points and returns the scale now in effect (0 when the
// view is not ready). Rust sets the Bevy window to points x scale itself:
// winit's resize event can be lost when this runs inside a frame.
float l4d2r_set_render_scale(float scale, float *width, float *height) {
    if (!NSThread.isMainThread) return 0;
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:UIWindowScene.class]) continue;
        for (UIWindow *window in ((UIWindowScene *)scene).windows) {
            UIView *view = window.rootViewController.view;
            if (!view) continue;
            CGFloat native = window.screen.nativeScale;
            CGFloat wanted = MIN(MAX(scale, 1.0), native);
            if (ABS(view.contentScaleFactor - wanted) > 0.01) {
                view.contentScaleFactor = wanted;
            }
            *width = (float)view.bounds.size.width;
            *height = (float)view.bounds.size.height;
            return (float)view.contentScaleFactor;
        }
    }
    return 0;
}

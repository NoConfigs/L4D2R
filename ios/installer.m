#import <UIKit/UIKit.h>
#import <CoreImage/CoreImage.h>
#import <Security/Security.h>
#include <stdint.h>
#include <string.h>

extern void l4d2r_install_start(int pack);
extern void l4d2r_install_login(int pack, const char *account, const char *password);
extern void l4d2r_install_guard(const char *code);
extern bool l4d2r_pack_complete(int pack);
extern void l4d2r_install_cancel(void);
extern void l4d2r_install_forget(void);

static NSDictionary *loginQuery(void) {
    return @{(__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService: @"com.l4d2r.steam",
        (__bridge id)kSecAttrAccount: @"installer"};
}
char *l4d2r_login_read(void) {
    NSMutableDictionary *q = [loginQuery() mutableCopy];
    q[(__bridge id)kSecReturnData] = @YES;
    CFTypeRef result = NULL;
    if (SecItemCopyMatching((__bridge CFDictionaryRef)q, &result) != errSecSuccess) return NULL;
    NSData *data = CFBridgingRelease(result);
    char *out = calloc(data.length + 1, 1);
    if (out) memcpy(out, data.bytes, data.length);
    return out;
}
bool l4d2r_login_save(const char *value) {
    NSData *data = [NSData dataWithBytes:value length:strlen(value)];
    NSDictionary *attributes = @{(__bridge id)kSecValueData: data,
        (__bridge id)kSecAttrAccessible: (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly};
    OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)loginQuery(), (__bridge CFDictionaryRef)attributes);
    if (status == errSecItemNotFound) {
        NSMutableDictionary *q = [loginQuery() mutableCopy]; [q addEntriesFromDictionary:attributes];
        status = SecItemAdd((__bridge CFDictionaryRef)q, NULL);
    }
    return status == errSecSuccess;
}
void l4d2r_login_delete(void) { SecItemDelete((__bridge CFDictionaryRef)loginQuery()); }
void l4d2r_secret_free(char *value) {
    if (value) { memset(value, 0, strlen(value)); free(value); }
}

@interface L4D2InstallerDelegate : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@property(nonatomic, strong) UILabel *status;
@property(nonatomic, strong) UILabel *details;
@property(nonatomic, strong) UIImageView *qr;
@property(nonatomic, strong) UIProgressView *progress;
@property(nonatomic, strong) UIButton *action;
@property(nonatomic, strong) UIButton *openSteam;
@property(nonatomic, strong) UIButton *forget;
@property(nonatomic, strong) NSURL *challenge;
@property(nonatomic) BOOL busy;
@property(nonatomic) BOOL modal;
@property(nonatomic) BOOL needsCredentials;
@property(nonatomic) int pack;
@property(nonatomic, strong) UIButton *packButton;
@property(nonatomic, strong) UIButton *close;
@property(nonatomic, weak) UIViewController *controller;
@property(nonatomic, strong) UIAlertController *guardAlert;
@property(nonatomic) uint64_t lastBytes;
@property(nonatomic) double lastTime;
@property(nonatomic) double rate;
- (UIViewController *)buildController;
- (void)setRunning:(BOOL)busy;
- (void)showGuard:(NSString *)message;
@end
static L4D2InstallerDelegate *installer;
static NSArray<NSString *> *packNames(void) {
    return @[@"Starter: shared assets + Dead Center", @"Dead Center", @"Dark Carnival", @"Swamp Fever", @"Hard Rain", @"The Parish", @"The Passing", @"The Sacrifice", @"No Mercy", @"Crash Course", @"Death Toll", @"Dead Air", @"Blood Harvest", @"Cold Stream", @"The Last Stand", @"Extra survivor dialogue", @"All game files"];
}

@implementation L4D2InstallerDelegate
- (UIButton *)button:(NSString *)title action:(SEL)selector {
    UIButton *b = [UIButton buttonWithType:UIButtonTypeSystem];
    [b setTitle:title forState:UIControlStateNormal];
    b.titleLabel.font = [UIFont boldSystemFontOfSize:18];
    [b addTarget:self action:selector forControlEvents:UIControlEventTouchUpInside];
    [b.heightAnchor constraintGreaterThanOrEqualToConstant:44].active = YES;
    return b;
}
- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)options {
    installer = self;
    self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];
    self.window.rootViewController = [self buildController];
    [self.window makeKeyAndVisible];
    return YES;
}
- (UIViewController *)buildController {
    UIViewController *controller = [UIViewController new];
    self.controller = controller;
    controller.modalInPresentation = YES;
    controller.view.backgroundColor = [UIColor colorWithWhite:0.06 alpha:1];
    UIScrollView *scroll = [UIScrollView new]; scroll.translatesAutoresizingMaskIntoConstraints = NO;
    [controller.view addSubview:scroll];
    UIStackView *stack = [UIStackView new]; stack.axis = UILayoutConstraintAxisVertical;
    stack.spacing = 12; stack.translatesAutoresizingMaskIntoConstraints = NO;
    [scroll addSubview:stack];
    UILabel *title = [UILabel new]; title.text = @"LEFT 4 DEAD 2 — GAME FILES";
    title.textColor = UIColor.whiteColor; title.font = [UIFont boldSystemFontOfSize:24]; title.numberOfLines = 0;
    self.status = [UILabel new]; self.status.textColor = UIColor.whiteColor; self.status.numberOfLines = 0;
    self.status.text = @"Sign in with your Steam account and approve Steam Guard on this phone. Your password is sent to Steam and is not saved.";
    self.details = [UILabel new]; self.details.textColor = UIColor.lightGrayColor; self.details.numberOfLines = 0;
    self.details.text = @"Start with shared assets and Dead Center. Other campaigns and extra dialogue can be downloaded later here. Shared archives still require several GB. Use Wi-Fi and keep the app open; completed downloads are kept.";
    self.qr = [UIImageView new]; self.qr.contentMode = UIViewContentModeScaleAspectFit; self.qr.hidden = YES;
    self.qr.accessibilityLabel = @"Steam Guard sign-in QR code";
    [self.qr.heightAnchor constraintEqualToConstant:240].active = YES;
    self.progress = [UIProgressView new]; self.progress.hidden = YES;
    self.action = [self button:@"Sign in with Steam / Resume" action:@selector(startOrPause)];
    self.openSteam = [self button:@"Open Steam to approve" action:@selector(approve)]; self.openSteam.hidden = YES;
    self.forget = [self button:@"Forget Steam sign-in" action:@selector(forgetLogin)];
    self.packButton = [self button:packNames()[self.pack] action:@selector(selectPack)];
    self.close = [self button:@"Back to game" action:@selector(closeFiles)]; self.close.hidden = !self.modal;
    for (UIView *v in @[title,self.packButton,self.status,self.qr,self.progress,self.details,self.action,self.openSteam,self.forget,self.close]) [stack addArrangedSubview:v];
    UILayoutGuide *safe = controller.view.safeAreaLayoutGuide;
    [NSLayoutConstraint activateConstraints:@[
        [scroll.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor], [scroll.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor],
        [scroll.topAnchor constraintEqualToAnchor:safe.topAnchor], [scroll.bottomAnchor constraintEqualToAnchor:safe.bottomAnchor],
        [stack.leadingAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.leadingAnchor constant:24],
        [stack.trailingAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.trailingAnchor constant:-24],
        [stack.topAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.topAnchor constant:20],
        [stack.bottomAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.bottomAnchor constant:-20],
        [stack.widthAnchor constraintEqualToAnchor:scroll.frameLayoutGuide.widthAnchor constant:-48]
    ]];
    return controller;
}
- (void)selectPack {
    UIAlertController *sheet = [UIAlertController alertControllerWithTitle:@"Game files" message:@"Starter includes common content and Dead Center. Other campaigns are optional." preferredStyle:UIAlertControllerStyleActionSheet];
    for (int n = 0; n < (int)packNames().count; ++n) {
        NSString *title = [packNames()[n] stringByAppendingString:l4d2r_pack_complete(n) ? @" — installed" : @""];
        [sheet addAction:[UIAlertAction actionWithTitle:title style:UIAlertActionStyleDefault handler:^(UIAlertAction *a) {
            self.pack = n; [self.packButton setTitle:packNames()[n] forState:UIControlStateNormal];
            self.action.hidden = NO; self.progress.hidden = YES;
            self.status.text = l4d2r_pack_complete(n) ? @"Installed. You can verify or repair these files by tapping Download / Resume." : @"Tap Download / Resume to install this pack. Shared content is reused.";
        }]];
    }
    [sheet addAction:[UIAlertAction actionWithTitle:@"Cancel" style:UIAlertActionStyleCancel handler:nil]];
    sheet.popoverPresentationController.sourceView = self.packButton;
    sheet.popoverPresentationController.sourceRect = self.packButton.bounds;
    [self.controller presentViewController:sheet animated:YES completion:nil];
}
- (void)closeFiles {
    if (!self.busy && self.modal) { [self.controller dismissViewControllerAnimated:YES completion:nil]; installer = nil; }
}
- (void)showLogin {
    UIAlertController *sheet = [UIAlertController alertControllerWithTitle:@"Sign in with Steam" message:@"Use your Steam account name. Your password goes to Steam and is not saved. Approve Steam Guard on this phone, or enter its code." preferredStyle:UIAlertControllerStyleAlert];
    [sheet addTextFieldWithConfigurationHandler:^(UITextField *t) {
        t.placeholder = @"Steam account name"; t.autocapitalizationType = UITextAutocapitalizationTypeNone;
        t.autocorrectionType = UITextAutocorrectionTypeNo; t.textContentType = UITextContentTypeUsername;
    }];
    [sheet addTextFieldWithConfigurationHandler:^(UITextField *t) {
        t.placeholder = @"Password"; t.secureTextEntry = YES; t.textContentType = UITextContentTypePassword;
    }];
    [sheet addAction:[UIAlertAction actionWithTitle:@"Cancel" style:UIAlertActionStyleCancel handler:nil]];
    [sheet addAction:[UIAlertAction actionWithTitle:@"Sign in" style:UIAlertActionStyleDefault handler:^(UIAlertAction *a) {
        NSString *account = sheet.textFields[0].text; NSString *password = sheet.textFields[1].text;
        if (!account.length || !password.length) { self.status.text = @"Enter your Steam account name and password."; return; }
        [self setRunning:YES]; self.needsCredentials = NO;
        l4d2r_install_login(self.pack, account.UTF8String, password.UTF8String);
        sheet.textFields[1].text = @"";
    }]];
    [self.controller presentViewController:sheet animated:YES completion:nil];
}
- (void)setRunning:(BOOL)busy {
    self.busy = busy; self.forget.enabled = !busy;
    self.packButton.enabled = !busy; self.close.enabled = !busy;
    UIApplication.sharedApplication.idleTimerDisabled = busy;
    [self.action setTitle:busy ? @"Pause download" : @"Download / Resume" forState:UIControlStateNormal];
    self.action.enabled = YES;
    if (busy) { self.lastTime = 0; self.rate = 0; }
}
- (void)startOrPause {
    if (self.busy) {
        self.action.enabled = NO; self.status.text = @"Pausing…";
        l4d2r_install_cancel();
    } else {
        char *saved = l4d2r_login_read(); BOOL hasLogin = saved != NULL;
        l4d2r_secret_free(saved);
        if (!hasLogin || self.needsCredentials) { [self showLogin]; return; }
        [self setRunning:YES]; self.qr.hidden = YES; self.openSteam.hidden = YES;
        self.progress.hidden = YES; self.status.text = @"Connecting to Steam…";
        l4d2r_install_start(self.pack);
    }
}
- (void)approve {
    [UIApplication.sharedApplication openURL:self.challenge ?: [NSURL URLWithString:@"steam://"] options:@{} completionHandler:nil];
}
- (void)forgetLogin {
    l4d2r_install_forget(); self.qr.hidden = YES; self.openSteam.hidden = YES; self.challenge = nil;
    self.status.text = @"Saved sign-in removed from this iPhone. Tap Sign in to use another account.";
}
- (void)showGuard:(NSString *)message {
    self.status.text = message; self.openSteam.hidden = NO;
    UIAlertController *alert = [UIAlertController alertControllerWithTitle:@"Steam Guard" message:message preferredStyle:UIAlertControllerStyleAlert];
    self.guardAlert = alert;
    [alert addTextFieldWithConfigurationHandler:^(UITextField *t) {
        t.placeholder = @"Steam Guard / email code"; t.autocapitalizationType = UITextAutocapitalizationTypeAllCharacters;
        t.autocorrectionType = UITextAutocorrectionTypeNo; t.textContentType = UITextContentTypeOneTimeCode;
    }];
    [alert addAction:[UIAlertAction actionWithTitle:@"Cancel sign-in" style:UIAlertActionStyleCancel handler:^(UIAlertAction *a) { l4d2r_install_cancel(); self.guardAlert = nil; }]];
    [alert addAction:[UIAlertAction actionWithTitle:@"Approve in Steam app" style:UIAlertActionStyleDefault handler:^(UIAlertAction *a) { self.guardAlert = nil; [self approve]; }]];
    [alert addAction:[UIAlertAction actionWithTitle:@"Submit code" style:UIAlertActionStyleDefault handler:^(UIAlertAction *a) {
        NSString *code = alert.textFields.firstObject.text;
        if (code.length) l4d2r_install_guard(code.UTF8String);
        self.guardAlert = nil;
    }]];
    [self.controller presentViewController:alert animated:YES completion:nil];
}
- (void)applicationDidEnterBackground:(UIApplication *)application {
    // Switching to Steam during QR approval must not cancel that approval.
    // iOS suspends this process; polling continues when it becomes active.
    UIApplication.sharedApplication.idleTimerDisabled = NO;
}
- (void)applicationDidBecomeActive:(UIApplication *)application {
    application.idleTimerDisabled = self.busy;
}
@end

void l4d2r_installer_event(int kind, const char *text, uint64_t done, uint64_t total) {
    NSString *message = [NSString stringWithUTF8String:text];
    dispatch_async(dispatch_get_main_queue(), ^{
        L4D2InstallerDelegate *i = installer; if (!i) return;
        if (kind == 6) {
            [i showGuard:message];
        } else if (kind == 7) {
            i.needsCredentials = YES; i.status.text = message;
        } else if (kind == 1) {
            NSURL *url = [NSURL URLWithString:message];
            if (![url.scheme isEqualToString:@"https"] || ![url.host isEqualToString:@"s.team"] || ![url.path hasPrefix:@"/q/"]) return;
            i.challenge = url;
            CIFilter *filter = [CIFilter filterWithName:@"CIQRCodeGenerator"];
            [filter setValue:[message dataUsingEncoding:NSUTF8StringEncoding] forKey:@"inputMessage"];
            [filter setValue:@"M" forKey:@"inputCorrectionLevel"];
            CIImage *image = [filter.outputImage imageByApplyingTransform:CGAffineTransformMakeScale(8, 8)];
            CGImageRef cg = [[CIContext contextWithOptions:nil] createCGImage:image fromRect:image.extent];
            if (cg) {
                UIImage *code = [UIImage imageWithCGImage:cg]; CGImageRelease(cg);
                // Preserve a white quiet zone so Steam can reliably scan it.
                CGSize size = CGSizeMake(code.size.width + 64, code.size.height + 64);
                UIGraphicsImageRenderer *renderer = [[UIGraphicsImageRenderer alloc] initWithSize:size];
                i.qr.image = [renderer imageWithActions:^(UIGraphicsImageRendererContext *context) {
                    [UIColor.whiteColor setFill]; UIRectFill(CGRectMake(0, 0, size.width, size.height));
                    CGContextSetInterpolationQuality(context.CGContext, kCGInterpolationNone);
                    [code drawAtPoint:CGPointMake(32, 32)];
                }];
            }
            i.qr.backgroundColor = UIColor.whiteColor; i.qr.hidden = NO; i.openSteam.hidden = NO;
            i.status.text = @"Approve this sign-in in Steam Guard. Scan the QR with another device, or try Open Steam on this iPhone.";
        } else if (kind == 2) {
            i.qr.hidden = YES; i.openSteam.hidden = YES; i.challenge = nil;
            i.progress.hidden = NO; i.progress.progress = total ? (float)((double)done / total) : 0;
            i.status.text = [NSString stringWithFormat:@"Installing assets — %.1f%%", total ? (double)done * 100 / total : 0];
            BOOL checking = [message hasPrefix:@"Checking "];
            double now = CFAbsoluteTimeGetCurrent();
            if (checking || done < i.lastBytes) { i.lastTime = 0; i.rate = 0; }
            if (i.lastTime > 0 && now - i.lastTime > 0.1) {
                double sample = (double)(done - i.lastBytes) / (now - i.lastTime);
                i.rate = i.rate > 0 ? i.rate * 0.6 + sample * 0.4 : sample;
            }
            i.lastBytes = done; i.lastTime = checking ? 0 : now;
            NSString *speed = checking ? @"Verifying existing files" : i.rate > 0 ? [NSString stringWithFormat:@"%.1f MB/s — about %.0f min remaining", i.rate/1e6, (double)(total-done)/i.rate/60] : @"Downloading";
            i.details.text = [NSString stringWithFormat:@"%.2f / %.2f GB • %@\n%@\nKeep the app open. Verified files and partial downloads are retained.", (double)done/1e9, (double)total/1e9, speed, message];
        } else {
            i.status.text = message;
            if (kind == 0 && [message hasPrefix:@"Approve"]) i.openSteam.hidden = NO;
            if (i.guardAlert && !(kind == 0 && [message hasPrefix:@"Approve"])) {
                [i.guardAlert dismissViewControllerAnimated:YES completion:nil]; i.guardAlert = nil;
            }
            if (kind == 3 || kind == 4) {
                [i setRunning:NO]; i.qr.hidden = YES; i.openSteam.hidden = YES; i.challenge = nil;
                if (kind == 3) {
                    i.progress.progress = 1;
                    i.details.text = i.modal ? @"Pack installed. Tap Back to game; the new maps are ready to select." : @"Starter files are ready. Close and reopen the app to play. Download other packs later from GAME FILES in the main menu.";
                }
            }
        }
    });
}
void l4d2r_installer_run(void) {
    @autoreleasepool { UIApplicationMain(0, NULL, nil, NSStringFromClass(L4D2InstallerDelegate.class)); }
}
void l4d2r_game_files_show(int pack) {
    dispatch_async(dispatch_get_main_queue(), ^{
        if (installer) return;
        UIWindow *window = nil;
        for (UIWindow *w in UIApplication.sharedApplication.windows) if (w.isKeyWindow) { window = w; break; }
        UIViewController *presenter = window.rootViewController;
        while (presenter.presentedViewController) presenter = presenter.presentedViewController;
        if (!presenter) return;
        L4D2InstallerDelegate *manager = [L4D2InstallerDelegate new];
        manager.modal = YES; manager.pack = MAX(0, MIN(16, pack));
        installer = manager;
        UIViewController *controller = [manager buildController];
        controller.modalPresentationStyle = UIModalPresentationFullScreen;
        [presenter presentViewController:controller animated:YES completion:nil];
    });
}

#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>

// A development site gets normal WebKit rendering and input, with no native JS bridge.
@interface UmideBrowser : WKWebView <WKNavigationDelegate>
@property(nonatomic, strong) NSURL *origin;
@end

@implementation UmideBrowser
- (void)webView:(WKWebView *)view decidePolicyForNavigationAction:(WKNavigationAction *)action
    decisionHandler:(void (^)(WKNavigationActionPolicy))decide {
    NSURL *url = action.request.URL;
    BOOL same = [url.scheme isEqualToString:self.origin.scheme]
        && [url.host isEqualToString:self.origin.host]
        && [url.port isEqual:self.origin.port];
    if (action.targetFrame && !action.targetFrame.mainFrame) {
        decide(WKNavigationActionPolicyAllow);
    } else if (same && action.targetFrame) {
        decide(WKNavigationActionPolicyAllow);
    } else {
        decide(WKNavigationActionPolicyCancel);
        if (same && !action.targetFrame) [view loadRequest:action.request];
    }
}
@end

void *umide_browser_new(void *nativeParent, const char *address) {
    NSView *parent = (__bridge NSView *)nativeParent;
    NSURL *url = [NSURL URLWithString:[NSString stringWithUTF8String:address]];
    if (![url.scheme isEqualToString:@"http"] ||
        !([url.host isEqualToString:@"127.0.0.1"] || [url.host isEqualToString:@"localhost"] || [url.host isEqualToString:@"::1"]) ||
        !url.port || url.user || url.password) return NULL;
    WKWebViewConfiguration *config = [[WKWebViewConfiguration alloc] init];
    config.websiteDataStore = WKWebsiteDataStore.nonPersistentDataStore;
    UmideBrowser *view = [[UmideBrowser alloc] initWithFrame:NSZeroRect configuration:config];
    view.origin = url;
    view.navigationDelegate = view;
    if (@available(macOS 13.3, *)) view.inspectable = YES;
    [parent addSubview:view];
    [view loadRequest:[NSURLRequest requestWithURL:url]];
    return (__bridge_retained void *)view;
}

void umide_browser_layout(void *handle, double x, double y, double w, double h) {
    UmideBrowser *view = (__bridge UmideBrowser *)handle;
    NSView *parent = view.superview;
    NSRect frame = NSMakeRect(x, parent.isFlipped ? y : NSHeight(parent.bounds) - y - h, w, h);
    if (!NSEqualRects(view.frame, frame)) view.frame = frame;
}

void umide_browser_reload(void *handle) {
    [(__bridge UmideBrowser *)handle reloadFromOrigin];
}

void umide_browser_remove(void *handle) {
    UmideBrowser *view = (__bridge_transfer UmideBrowser *)handle;
    view.navigationDelegate = nil;
    [view stopLoading];
    [view removeFromSuperview];
}

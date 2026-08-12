#import <Foundation/Foundation.h>
#include <stdbool.h>

typedef void (*ZlackNotificationCallback)(const char *notification_id,
                                          bool activated);

static ZlackNotificationCallback zlack_callback = NULL;

@interface ZlackNotificationDelegate
    : NSObject <NSUserNotificationCenterDelegate>
@end

@implementation ZlackNotificationDelegate

- (BOOL)userNotificationCenter:(NSUserNotificationCenter *)center
     shouldPresentNotification:(NSUserNotification *)notification {
  return YES;
}

- (void)userNotificationCenter:(NSUserNotificationCenter *)center
       didActivateNotification:(NSUserNotification *)notification {
  if (zlack_callback != NULL && notification.identifier.length > 0) {
    zlack_callback(notification.identifier.UTF8String, true);
  }
  [center removeDeliveredNotification:notification];
}

- (void)userNotificationCenter:(NSUserNotificationCenter *)center
               didDismissAlert:(NSUserNotification *)notification {
  if (zlack_callback != NULL && notification.identifier.length > 0) {
    zlack_callback(notification.identifier.UTF8String, false);
  }
}

@end

void zlack_notification_initialize(ZlackNotificationCallback callback) {
  zlack_callback = callback;
  dispatch_async(dispatch_get_main_queue(), ^{
    static ZlackNotificationDelegate *delegate;
    static dispatch_once_t once_token;
    dispatch_once(&once_token, ^{
      delegate = [[ZlackNotificationDelegate alloc] init];
    });
    [NSUserNotificationCenter defaultUserNotificationCenter].delegate = delegate;
  });
}

void zlack_notification_show(const char *notification_id, const char *title,
                             const char *body) {
  NSString *identifier =
      [NSString stringWithUTF8String:notification_id ?: ""];
  NSString *notification_title = [NSString stringWithUTF8String:title ?: ""];
  NSString *notification_body = [NSString stringWithUTF8String:body ?: ""];

  dispatch_async(dispatch_get_main_queue(), ^{
    NSUserNotification *notification = [[NSUserNotification alloc] init];
    notification.identifier = identifier;
    notification.title = notification_title;
    notification.informativeText = notification_body;
    notification.soundName = NSUserNotificationDefaultSoundName;
    [[NSUserNotificationCenter defaultUserNotificationCenter]
        deliverNotification:notification];
  });
}

//! Apple scheduling hints for the shell and its owned worker threads.

#[repr(u32)]
enum QualityOfService {
    UserInteractive = 0x21,
    Utility = 0x11,
    Background = 0x09,
}

pub(super) fn interactive() {
    set(QualityOfService::UserInteractive);
}

pub(super) fn utility() {
    set(QualityOfService::Utility);
}

pub(super) fn background() {
    set(QualityOfService::Background);
}

fn set(class: QualityOfService) {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
    }
    // SAFETY: These are the SDK's QoS class values and the default relative priority.
    // A rejected hint leaves the thread running at its previous priority.
    unsafe { pthread_set_qos_class_self_np(class as u32, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_threads_receive_the_requested_class() {
        unsafe extern "C" {
            fn qos_class_self() -> u32;
        }

        for (setup, expected) in [
            (interactive as fn(), QualityOfService::UserInteractive),
            (utility, QualityOfService::Utility),
            (background, QualityOfService::Background),
        ] {
            let actual = std::thread::spawn(move || {
                setup();
                // SAFETY: The function reads the calling thread's QoS and takes no arguments.
                unsafe { qos_class_self() }
            })
            .join()
            .unwrap();
            assert_eq!(actual, expected as u32);
        }
    }
}

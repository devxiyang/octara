use octara::prelude::*;

fn main() -> Result<(), octara_host_native::ShellError> {
    let definition = AppDefinition::new().window(WindowDefinition::new(WindowOptions::default(), |cx| {
        let field = cx.signal(String::from("Octara v2"));
        col()
            .p(24)
            .gap(8)
            .child(text("Native presentation").text_size(22))
            .child(input().value(field).test_id("name").autofocus().on_input(move |write, value| write.set(field, value.into())))
    }));
    octara_host_native::run_app_with_first_frame(definition, |receipt| {
        assert!(receipt.gpu_complete);
        println!("presented frame {}", receipt.stamp.frame);
    })
}

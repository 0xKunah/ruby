//! Embeds the Ruby icon and version info into the Windows executable. The icon gets resource
//! ID 1, which is also what GPUI loads for the window and taskbar icon.

fn main() {
	println!("cargo:rerun-if-changed=assets/ruby.ico");
	if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
		let mut res = winresource::WindowsResource::new();
		res.set_icon("assets/ruby.ico")
			.set("ProductName", "Ruby")
			.set("FileDescription", "Ruby - a fast, lightweight notes app")
			.set("CompanyName", "Ruby");
		if let Err(e) = res.compile() {
			panic!("failed to embed the Windows icon: {e}");
		}
	}
}

//! Gives the helper Mesh's icon and a proper name in Task Manager.

fn main() {
    println!("cargo:rerun-if-changed=../crates/mesh-app/assets/mesh.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../crates/mesh-app/assets/mesh.ico")
            .set("ProductName", "Mesh")
            .set("FileDescription", "Mesh Discord")
            .set("CompanyName", "Mesh")
            .set("OriginalFilename", "mesh-dc.exe");
        if let Err(err) = res.compile() {
            println!("cargo:warning=no exe icon: {err}");
        }
    }
}

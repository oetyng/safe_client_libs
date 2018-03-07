// Copyright 2017 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under (1) the MaidSafe.net Commercial License,
// version 1.0 or later, or (2) The General Public License (GPL), version 3, depending on which
// licence you accepted on initial access to the Software (the "Licences").
//
// By contributing code to the SAFE Network Software, or to this project generally, you agree to be
// bound by the terms of the MaidSafe Contributor Agreement.  This, along with the Licenses can be
// found in the root directory of this project at LICENSE, COPYING and CONTRIBUTOR.
//
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.
//
// Please review the Licences for the specific language governing permissions and limitations
// relating to use of the SAFE Network Software.

//! Build script for generating C header files from FFI modules.

extern crate ffi_utils;
extern crate rust_sodium;
extern crate routing;
extern crate safe_bindgen;
#[macro_use]
extern crate unwrap;

use routing::XOR_NAME_LEN;
use rust_sodium::crypto::{box_, secretbox, sign};
use safe_bindgen::{Bindgen, FilterMode, LangCSharp, LangJava};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

fn main() {
    if env::var("CARGO_FEATURE_BINDINGS").is_err() {
        return;
    }

    gen_bindings_c();
    gen_bindings_csharp();
    gen_bindings_java();
}

fn gen_bindings_c() {
    unwrap!(ffi_utils::header_gen::gen_headers(
        &unwrap!(env::var("CARGO_PKG_NAME")),
        "../bindings/c/",
        "src/lib.rs",
    ));
}

fn gen_bindings_java() {
    let target_dir = Path::new("../bindings/java/safe_app");

    let mut type_map = HashMap::new();
    type_map.insert("XorNameArray", "byte[]");
    type_map.insert("SignSecretKey", "byte[]");
    type_map.insert("SignPublicKey", "byte[]");
    type_map.insert("SymSecretKey", "byte[]");
    type_map.insert("SymNonce", "byte[]");
    type_map.insert("AsymPublicKey", "byte[]");
    type_map.insert("AsymSecretKey", "byte[]");
    type_map.insert("AsymNonce", "byte[]");
    type_map.insert("CipherOptHandle", "long");
    type_map.insert("EncryptPubKeyHandle", "long");
    type_map.insert("EncryptSecKeyHandle", "long");
    type_map.insert("MDataEntriesHandle", "long");
    type_map.insert("MDataEntryActionsHandle", "long");
    type_map.insert("MDataPermissionsHandle", "long");
    type_map.insert("SelfEncryptorReaderHandle", "long");
    type_map.insert("SelfEncryptorWriterHandle", "long");
    type_map.insert("SEReaderHandle", "long");
    type_map.insert("SEWriterHandle", "long");
    type_map.insert("SignPubKeyHandle", "long");
    type_map.insert("SignSecKeyHandle", "long");
    type_map.insert("FileContextHandle", "long");
    type_map.insert("App", "long");
    type_map.insert("Authenticator", "long");

    let mut bindgen = unwrap!(Bindgen::new());
    let mut lang = LangJava::new(type_map);

    lang.set_namespace("net.maidsafe.safe_app");

    let mut outputs = HashMap::new();

    bindgen.source_file("../safe_core/src/lib.rs");
    lang.set_lib_name("safe_core");
    unwrap!(bindgen.compile(&mut lang, &mut outputs, false));

    bindgen.source_file("../ffi_utils/src/lib.rs");
    lang.set_lib_name("ffi_utils");
    unwrap!(bindgen.compile(&mut lang, &mut outputs, false));

    bindgen.source_file("src/lib.rs");
    lang.set_lib_name(unwrap!(env::var("CARGO_PKG_NAME")));
    unwrap!(bindgen.compile(&mut lang, &mut outputs, true));

    unwrap!(bindgen.write_outputs(target_dir, &outputs));
}

fn gen_bindings_csharp() {
    let target_dir = Path::new("../bindings/csharp/safe_app");
    let test_idents = ["test_create_app", "test_create_app_with_access"];

    let mut bindgen = unwrap!(Bindgen::new());
    let mut lang = LangCSharp::new();

    lang.set_lib_name(unwrap!(env::var("CARGO_PKG_NAME")));

    lang.set_interface_section(
        "SafeApp.Utilities/IAppBindings.cs",
        "SafeApp.Utilities",
        "IAppBindings",
    );
    lang.set_functions_section(
        "SafeApp.AppBindings/AppBindings.cs",
        "SafeApp.AppBindings",
        "AppBindings",
    );
    lang.set_consts_section(
        "SafeApp.Utilities/AppConstants.cs",
        "SafeApp.Utilities",
        "AppConstants",
    );
    lang.set_types_section("SafeApp.Utilities/AppTypes.cs", "SafeApp.Utilities");
    lang.set_utils_section(
        "SafeApp.Utilities/BindingUtils.cs",
        "SafeApp.Utilities",
        "BindingUtils",
    );

    lang.add_const("ulong", "ASYM_PUBLIC_KEY_LEN", box_::PUBLICKEYBYTES);
    lang.add_const("ulong", "ASYM_SECRET_KEY_LEN", box_::SECRETKEYBYTES);
    lang.add_const("ulong", "ASYM_NONCE_LEN", box_::NONCEBYTES);
    lang.add_const("ulong", "SYM_KEY_LEN", secretbox::KEYBYTES);
    lang.add_const("ulong", "SYM_NONCE_LEN", secretbox::NONCEBYTES);
    lang.add_const("ulong", "SIGN_PUBLIC_KEY_LEN", sign::PUBLICKEYBYTES);
    lang.add_const("ulong", "SIGN_SECRET_KEY_LEN", sign::SECRETKEYBYTES);
    lang.add_const("ulong", "XOR_NAME_LEN", XOR_NAME_LEN);
    lang.add_opaque_type("App");

    lang.reset_filter(FilterMode::Blacklist);
    for &ident in &test_idents {
        lang.filter(ident);
    }

    bindgen.source_file("../safe_core/src/lib.rs");
    bindgen.compile_or_panic(&mut lang, &mut HashMap::new(), false);

    let mut outputs = HashMap::new();
    bindgen.source_file("src/lib.rs");
    bindgen.compile_or_panic(&mut lang, &mut outputs, true);
    apply_patches(&mut outputs);
    bindgen.write_outputs_or_panic(target_dir, &outputs);

    // Testing utilities.
    lang.set_interface_section(
        "SafeApp.MockAuthBindings/IMockAuthBindings.cs",
        "SafeApp.MockAuthBindings",
        "IMockAuthBindings",
    );
    lang.set_functions_section(
        "SafeApp.MockAuthBindings/MockAuthBindings.cs",
        "SafeApp.MockAuthBindings",
        "MockAuthBindings",
    );

    lang.set_consts_enabled(false);
    lang.set_types_enabled(false);
    lang.set_utils_enabled(false);
    lang.add_opaque_type("App");

    lang.reset_filter(FilterMode::Whitelist);
    for &ident in &test_idents {
        lang.filter(ident);
    }

    outputs.clear();
    bindgen.compile_or_panic(&mut lang, &mut outputs, true);
    apply_patches_testing(&mut outputs);
    bindgen.write_outputs_or_panic(target_dir, &outputs);

    // Hand-written code.
    unwrap!(ffi_utils::bindgen_utils::copy_files(
        "resources",
        target_dir,
        ".cs",
    ));
}

fn apply_patches(outputs: &mut HashMap<PathBuf, String>) {
    {
        let content = fetch_mut(outputs, "SafeApp.AppBindings/AppBindings.cs");
        insert_using_utilities(content);
        insert_using_obj_c_runtime(content);
        insert_guard(content);
        insert_resharper_disable_inconsistent_naming(content);
    }

    insert_internals_visible_to(fetch_mut(outputs, "SafeApp.Utilities/AppTypes.cs"));

    for content in outputs.values_mut() {
        fix_names(content);
    }
}

fn apply_patches_testing(outputs: &mut HashMap<PathBuf, String>) {
    insert_using_utilities(fetch_mut(
        outputs,
        "SafeApp.MockAuthBindings/MockAuthBindings.cs",
    ));

    insert_using_utilities(fetch_mut(
        outputs,
        "SafeApp.MockAuthBindings/IMockAuthBindings.cs",
    ));

    for content in outputs.values_mut() {
        fix_names(content);
    }
}

fn insert_guard(content: &mut String) {
    content.insert_str(0, "#if !NETSTANDARD1_2 || __DESKTOP__\n");
    content.push_str("#endif\n");
}


fn insert_using_utilities(content: &mut String) {
    content.insert_str(0, "using SafeApp.Utilities;\n");
}

fn insert_using_obj_c_runtime(content: &mut String) {
    content.insert_str(0, "#if __IOS__\nusing ObjCRuntime;\n#endif\n");
}

fn insert_internals_visible_to(content: &mut String) {
    content.insert_str(0, "using System.Runtime.CompilerServices;\n");

    // HACK: The [assembly: ...] annotation must go after all using declaration.
    *content = content.replace(
        "namespace SafeApp.Utilities",
        "[assembly: InternalsVisibleTo(\"SafeApp.AppBindings\")]\n\n\
         namespace SafeApp.Utilities",
    );
}

fn insert_resharper_disable_inconsistent_naming(content: &mut String) {
    content.insert_str(0, "// ReSharper disable InconsistentNaming\n");
}

fn fix_names(content: &mut String) {
    *content = content.replace("Idata", "IData").replace("Mdata", "MData");
}

fn fetch_mut<T: AsRef<Path>>(outputs: &mut HashMap<PathBuf, String>, key: T) -> &mut String {
    let key = key.as_ref();
    unwrap!(outputs.get_mut(key), "key {:?} not found in outputs", key)
}

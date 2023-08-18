/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use crate::{
    export::ImplItem,
    fnsig::{FnKind, FnSignature},
    util::{create_metadata_items, ident_to_string, mod_path, tagged_impl_header},
};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::iter;
use syn::{Ident, Path};

pub(super) fn trait_impl(
    ident: &Ident,
    trait_ident: &Ident,
    internals_ident: &Ident,
    items: &[ImplItem],
) -> syn::Result<TokenStream> {
    let trait_impl_methods = items
        .iter()
        .map(|item| match item {
            ImplItem::Method(sig) => gen_method_impl(sig, internals_ident),
            _ => unreachable!("traits have no constructors"),
        })
        .collect::<syn::Result<TokenStream>>()?;
    let ffi_converter_tokens = ffi_converter_callback_interface_impl(trait_ident, ident, None);

    Ok(quote! {
        #[doc(hidden)]
        #[derive(Debug)]
        struct #ident {
            handle: u64,
        }

        impl #ident {
            fn new(handle: u64) -> Self {
                Self { handle }
            }
        }

        impl ::std::ops::Drop for #ident {
            fn drop(&mut self) {
                #internals_ident.invoke_callback::<(), crate::UniFfiTag>(
                    self.handle, ::uniffi::IDX_CALLBACK_FREE, ::std::default::Default::default()
                )
            }
        }

        ::uniffi::deps::static_assertions::assert_impl_all!(#ident: Send);

        impl #trait_ident for #ident {
            #trait_impl_methods
        }

        #ffi_converter_tokens
    })
}

pub fn ffi_converter_callback_interface_impl(
    trait_ident: &Ident,
    trait_impl_ident: &Ident,
    tag: Option<&Path>,
) -> TokenStream {
    let name = ident_to_string(trait_ident);
    let impl_spec = tagged_impl_header("FfiConverter", &quote! { ::std::boxed::Box<dyn #trait_ident> }, tag);
    let tag = match tag {
        Some(t) => quote! { #t },
        None => quote! { T },
    };
    let mod_path = match mod_path() {
        Ok(p) => p,
        Err(e) => return e.into_compile_error(),
    };

    quote! {
        #[doc(hidden)]
        #[automatically_derived]
        unsafe #impl_spec {
            type FfiType = u64;

            // Lower and write are tricky to implement because we have a dyn trait as our type.  There's
            // probably a way to, but this carries lots of thread safety risks, down to impedance
            // mismatches between Rust and foreign languages, and our uncertainty around implementations of
            // concurrent handlemaps.
            //
            // The use case for them is also quite exotic: it's passing a foreign callback back to the foreign
            // language.
            //
            // Until we have some certainty, and use cases, we shouldn't use them.
            fn lower(_obj: Self) -> Self::FfiType {
                panic!("Lowering CallbackInterface not supported")
            }

            fn write(_obj: Self, _buf: &mut ::std::vec::Vec<u8>) {
                panic!("Writing CallbackInterface not supported")
            }

            fn try_lift(v: Self::FfiType) -> ::uniffi::deps::anyhow::Result<Self> {
                Ok(Box::new(<#trait_impl_ident>::new(v)))
            }

            fn try_read(buf: &mut &[u8]) -> ::uniffi::deps::anyhow::Result<Self> {
                use ::uniffi::deps::bytes::Buf;
                ::uniffi::check_remaining(buf, 8)?;
                <Self as ::uniffi::FfiConverter<crate::UniFfiTag>>::try_lift(buf.get_u64())
            }

            ::uniffi::ffi_converter_default_return!(#tag);

            const TYPE_ID_META: ::uniffi::MetadataBuffer = ::uniffi::MetadataBuffer::from_code(
                ::uniffi::metadata::codes::TYPE_CALLBACK_INTERFACE,
            )
            .concat_str(#mod_path)
            .concat_str(#name);
        }
    }
}

fn gen_method_impl(sig: &FnSignature, internals_ident: &Ident) -> syn::Result<TokenStream> {
    let FnSignature {
        ident,
        return_ty,
        kind,
        receiver,
        ..
    } = sig;
    let index = match kind {
        // Note: the callback index is 1-based, since 0 is reserved for the free function
        FnKind::TraitMethod { index, .. } => index + 1,
        k => {
            return Err(syn::Error::new(
                sig.span,
                format!(
                    "Internal UniFFI error: Unexpected function kind for callback interface {k:?}"
                ),
            ));
        }
    };

    if receiver.is_none() {
        return Err(syn::Error::new(
            sig.span,
            "callback interface methods must take &self as their first argument",
        ));
    }
    let params = sig.params();
    let buf_ident = Ident::new("uniffi_args_buf", Span::call_site());
    let write_exprs = sig.write_exprs(&buf_ident);

    Ok(quote! {
        fn #ident(&self, #(#params),*) -> #return_ty {
            #[allow(unused_mut)]
            let mut #buf_ident = ::std::vec::Vec::new();
            #(#write_exprs;)*
            let uniffi_args_rbuf = ::uniffi::RustBuffer::from_vec(#buf_ident);

            #internals_ident.invoke_callback::<#return_ty, crate::UniFfiTag>(self.handle, #index, uniffi_args_rbuf)
        }
    })
}

pub(super) fn metadata_items(
    self_ident: &Ident,
    items: &[ImplItem],
    module_path: &str,
) -> syn::Result<Vec<TokenStream>> {
    let trait_name = ident_to_string(self_ident);
    let callback_interface_items = create_metadata_items(
        "callback_interface",
        &trait_name,
        quote! {
            ::uniffi::MetadataBuffer::from_code(::uniffi::metadata::codes::CALLBACK_INTERFACE)
                .concat_str(#module_path)
                .concat_str(#trait_name)
        },
        None,
    );

    iter::once(Ok(callback_interface_items))
        .chain(items.iter().map(|item| match item {
            ImplItem::Method(sig) => sig.metadata_items(),
            _ => unreachable!("traits have no constructors"),
        }))
        .collect()
}

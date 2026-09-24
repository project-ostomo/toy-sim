use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemTrait, Pat, ReturnType, TraitItem, parse_macro_input};

/// Generate methods on this crate's OsgNetClient and a picomux dispatcher.
#[proc_macro_attribute]
pub fn rpc(_attribute: TokenStream, input: TokenStream) -> TokenStream {
    let service = parse_macro_input!(input as ItemTrait);
    match expand(service) {
        Ok(output) => output.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand(mut service: ItemTrait) -> syn::Result<proc_macro2::TokenStream> {
    if !service.generics.params.is_empty() || !service.supertraits.is_empty() {
        return Err(syn::Error::new_spanned(
            &service,
            "RPC traits cannot have generics or supertraits",
        ));
    }
    let name = service.ident.clone();
    let dispatcher = format_ident!("{}Dispatcher", name);
    let visibility = &service.vis;
    let mut methods = Vec::new();
    let mut arms = Vec::new();

    for item in &mut service.items {
        let TraitItem::Fn(method) = item else {
            return Err(syn::Error::new_spanned(
                item,
                "expected an async RPC method",
            ));
        };
        let signature = &method.sig;
        if signature.asyncness.is_none()
            || !signature.generics.params.is_empty()
            || signature.generics.where_clause.is_some()
            || signature.unsafety.is_some()
            || method.default.is_some()
        {
            return Err(syn::Error::new_spanned(
                signature,
                "RPC methods must be async, non-generic declarations",
            ));
        }
        let Some(FnArg::Receiver(receiver)) = signature.inputs.first() else {
            return Err(syn::Error::new_spanned(
                signature,
                "RPC methods require &self",
            ));
        };
        if receiver.reference.is_none()
            || receiver.mutability.is_some()
            || receiver.colon_token.is_some()
        {
            return Err(syn::Error::new_spanned(
                receiver,
                "RPC methods require &self",
            ));
        }
        let mut arguments = Vec::new();
        let mut types = Vec::new();
        for argument in signature.inputs.iter().skip(1) {
            let FnArg::Typed(argument) = argument else {
                unreachable!()
            };
            let Pat::Ident(pattern) = &*argument.pat else {
                return Err(syn::Error::new_spanned(
                    argument,
                    "RPC arguments require names",
                ));
            };
            if matches!(&*argument.ty, syn::Type::Reference(_)) {
                return Err(syn::Error::new_spanned(
                    argument,
                    "RPC arguments must be owned",
                ));
            }
            arguments.push(pattern.ident.clone());
            types.push(argument.ty.clone());
        }
        let result = match &signature.output {
            ReturnType::Default => quote!(()),
            ReturnType::Type(_, ty) => quote!(#ty),
        };
        let method_name = &signature.ident;
        let label = method_name.to_string();
        let documentation = &method.attrs;
        methods.push(quote! {
            #(#documentation)*
            pub async fn #method_name(&self, #(#arguments: #types),*) -> Result<#result, crate::RpcError> {
                let bytes = crate::rpc::encode(&(#(#arguments,)*), crate::rpc::MAX_REQUEST)?;
                let reply = self.rpc_call(#label, bytes).await?;
                crate::rpc::decode(&reply)
            }
        });
        arms.push(quote! {
            #label => {
                let (#(#arguments,)*) : (#(#types,)*) = crate::rpc::decode(&bytes)?;
                let result = self.0.#method_name(#(#arguments),*).await;
                let bytes = crate::rpc::encode(&result, crate::rpc::MAX_RESPONSE)?;
                crate::rpc::write(&mut stream, &bytes).await
            }
        });
        method.sig.asyncness = None;
        method.sig.output =
            syn::parse2(quote!(-> impl std::future::Future<Output = #result> + Send))?;
    }

    Ok(quote! {
        #service

        impl crate::OsgNetClient {
            #(#methods)*
        }

        #visibility struct #dispatcher<H>(pub H);

        impl<H: #name + Sync> #dispatcher<H> {
            pub async fn dispatch(&self, mut stream: crate::picomux::Stream) -> Result<(), crate::RpcError> {
                let method = std::str::from_utf8(stream.metadata())
                    .map_err(|error| crate::RpcError::Protocol(error.to_string()))?
                    .strip_prefix("rpc:")
                    .ok_or_else(|| crate::RpcError::Protocol("expected RPC stream".into()))?
                    .to_owned();
                let bytes = crate::rpc::read(&mut stream, crate::rpc::MAX_REQUEST).await?;
                match method.as_str() {
                    #(#arms,)*
                    _ => Err(crate::RpcError::Protocol(format!("unknown RPC method: {method}"))),
                }
            }
        }
    })
}

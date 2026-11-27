use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use syn::visit_mut::{self, VisitMut};
use syn::{Data, DeriveInput, Field, Fields, Type, parse_quote};

pub(super) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let visibility = &input.vis;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(name, "`Store` requires a struct with named fields"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(name, "`Store` requires a struct with named fields"));
    };
    let accessors = format_ident!("__OctaraStoreAccessorsFor{name}");
    let runtime = quote!(::octara::runtime::graph::store);
    let (_, arguments, _) = input.generics.split_for_impl();
    let owner: Type = parse_quote!(#name #arguments);
    let mut schema = input.generics.clone();
    schema.make_where_clause().predicates.push(parse_quote!(Self: 'static));
    let mut counts = Vec::with_capacity(fields.named.len());
    let mut definitions = Vec::with_capacity(fields.named.len());
    let mut methods = Vec::with_capacity(fields.named.len());

    for (index, field) in fields.named.iter().enumerate() {
        let name = field.ident.as_ref().expect("named field");
        let visibility = &field.vis;
        let ty = &field.ty;
        let mut method_ty = ty.clone();
        ReplaceSelf(&owner).visit_type_mut(&mut method_ty);
        let projection = quote! {
            |value| &value.downcast_ref::<Self>().expect("store parent type mismatch").#name,
            |value| &mut value.downcast_mut::<Self>().expect("store parent type mismatch").#name,
        };
        let index = Literal::usize_unsuffixed(index);
        let offset = quote!(<#owner as #runtime::StoreSchema>::OFFSETS[#index]);
        let (return_type, body) = if is_nested(field)? {
            schema.make_where_clause().predicates.push(parse_quote!(#ty: #runtime::StoreSchema));
            counts.push(quote!(<#ty as #runtime::StoreSchema>::NODE_COUNT));
            definitions.push(quote!(builder.nested::<#ty>(#projection);));
            (quote!(#runtime::Store<#method_ty>), quote!(#runtime::Store::from_field(self.0.child(#offset))))
        } else {
            schema.make_where_clause().predicates.push(parse_quote!(#ty: ::core::cmp::PartialEq + 'static));
            counts.push(quote!(1));
            definitions.push(quote!(builder.leaf::<#ty>(#projection);));
            (quote!(#runtime::Field<#method_ty>), quote!(self.0.child(#offset)))
        };
        let docs = field.attrs.iter().filter(|attr| attr.path().is_ident("doc"));
        methods.push(quote! {
            #(#docs)*
            #visibility fn #name(&self) -> #return_type {
                #body
            }
        });
    }

    let count = Literal::usize_unsuffixed(counts.len());
    let last_offset = Literal::usize_unsuffixed(counts.len() + 1);
    let mut projections = schema.clone();
    ReplaceSelf(&owner).visit_generics_mut(&mut projections);
    let (schema_parameters, _, schema_constraints) = schema.split_for_impl();
    let (projection_parameters, _, projection_constraints) = projections.split_for_impl();
    Ok(quote! {
        const _: () = {
            #[doc(hidden)]
            #visibility struct #accessors<__OctaraModel>(#runtime::Field<__OctaraModel>);

            impl<__OctaraModel> ::core::marker::Copy for #accessors<__OctaraModel> {}

            impl<__OctaraModel> ::core::clone::Clone for #accessors<__OctaraModel> {
                fn clone(&self) -> Self { *self }
            }

            impl<__OctaraModel> #runtime::StoreAccessors<__OctaraModel> for #accessors<__OctaraModel> {
                fn from_field(field: #runtime::Field<__OctaraModel>) -> Self { Self(field) }
                fn as_field(self) -> #runtime::Field<__OctaraModel> { self.0 }
            }

            impl #projection_parameters #accessors<#owner> #projection_constraints {
                #(#methods)*
            }

            impl #schema_parameters #runtime::StoreSchema for #owner #schema_constraints {
                type Accessors = #accessors<Self>;
                const OFFSETS: &'static [usize] = &{
                    let counts: [usize; #count] = [#(#counts),*];
                    let mut offsets = [1; #last_offset];
                    let mut index = 0;
                    while index < counts.len() {
                        offsets[index + 1] = offsets[index] + counts[index];
                        index += 1;
                    }
                    offsets
                };

                fn define(builder: &mut #runtime::StoreBuilder<'_, '_>) {
                    #(#definitions)*
                }
            }
        };
    })
}

fn is_nested(field: &Field) -> syn::Result<bool> {
    let mut nested = false;
    for attr in field.attrs.iter().filter(|attr| attr.path().is_ident("store")) {
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("nested") {
                return Err(meta.error("expected `nested`"));
            }
            if nested {
                return Err(meta.error("duplicate `nested` attribute"));
            }
            nested = true;
            Ok(())
        })?;
    }
    Ok(nested)
}

struct ReplaceSelf<'a>(&'a Type);

impl VisitMut for ReplaceSelf<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if matches!(ty, Type::Path(path) if path.qself.is_none() && path.path.is_ident("Self")) {
            *ty = self.0.clone();
        } else {
            visit_mut::visit_type_mut(self, ty);
        }
    }
}

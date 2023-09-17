#![feature(async_closure)]

// use std::future::Future;

use askama::Template;
use axum::{
    body::StreamBody,
    extract::{Form, FromRef, Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get, post},
    Router,
};
use axum_flash::{self, Flash, IncomingFlashes, Key};
use email_address::{self, EmailAddress};
use serde::Deserialize;
// use std::str::FromStr;

use learn_htmx::{Contact, ContactsTemplate, EditTemplate, NewTemplate, ViewTemplate, DB};

async fn view(
    State(state): State<AppState>,
    flashes: IncomingFlashes,
    Path(id): Path<u32>,
) -> (IncomingFlashes, Html<String>) {
    let c = state.db.get_contact(id).await.expect("could not get {id}");
    let messages: Box<_> = flashes.iter().map(|(_, txt)| txt).collect();
    let view = ViewTemplate::with_messages(&messages, c);
    let html = view.render().unwrap().into();
    (flashes, html)
}

async fn get_new() -> Html<String> {
    let view = NewTemplate::new("Full Name", "name@example.org", None)
        .render()
        .unwrap();
    view.into()
}

async fn get_edit(
    State(state): State<AppState>,
    flashes: IncomingFlashes,
    Path(id): Path<u32>,
) -> Html<String> {
    let c = state.db.get_contact(id).await.expect("could not get {id}");
    let messages: Box<_> = flashes.iter().map(|(_, txt)| txt).collect();
    let edit = EditTemplate::new(&messages, None, c);
    match edit.render() {
        Ok(html) => html.into(),
        Err(e) => format!("failed to render ViewTemplate\n{:?}", e).into(),
    }
}

#[derive(Deserialize, Debug)]
// #[allow(dead_code)]
struct Input {
    name: String,
    email: String,
}

async fn post_new(
    State(state): State<AppState>,
    flash: Flash,
    Form(input): Form<Input>,
) -> Result<(Flash, Redirect), NewContactError> {
    let email_res = EmailAddress::from_str(&input.email);
    match email_res {
        Ok(_) => (),
        Err(e) => {
            return Err(NewContactError {
                msg: e.to_string(),
                ui: input,
            })
        }
    };
    let op_id = state.db.find_email(&input.email).await.unwrap();
    if op_id.is_some() {
        return Err(NewContactError {
            msg: "This email is already occupied".to_string(),
            ui: input,
        });
    };
    state
        .db
        .add_contact(input.name.to_string(), input.email.to_string())
        .await
        .unwrap();
    Ok((flash.debug("New Contact Saved"), Redirect::to("/contacts")))
}

async fn post_edit(
    State(state): State<AppState>,
    flash: Flash,
    Path(id): Path<u32>,
    Form(input): Form<Input>,
) -> EditResult {
    let email_res = EmailAddress::from_str(&input.email);
    if let Err(e) = email_res {
        {
            return EditResult::Error {
                id,
                msg: e.to_string(),
                ui: input,
            };
        }
    };
    let op_id = state.db.find_email(&input.email).await.unwrap();
    if let Some(old_id) = op_id {
        if old_id as u32 != id {
            return EditResult::Error {
                id,
                msg: "This email is already occupied".to_string(),
                ui: input,
            };
        }
    };

    if let Err(e) = state.db.edit_contact(id, &input.name, &input.email).await {
        panic!("{}", e);
    };

    EditResult::Ok(id, flash.success("Changed Saved"))
}

struct NewContactError {
    msg: String,
    ui: Input,
}
impl IntoResponse for NewContactError {
    fn into_response(self) -> Response {
        let view = NewTemplate::new(&self.ui.name, &self.ui.email, Some(self.msg))
            .render()
            .unwrap();
        let html = Html::from(view);
        html.into_response()
    }
}

enum EditResult {
    Ok(u32, Flash),
    Error { id: u32, msg: String, ui: Input },
}
impl IntoResponse for EditResult {
    fn into_response(self) -> Response {
        match self {
            EditResult::Ok(id, flash) => {
                let re = Redirect::to(&format!("/contacts/{}", id));
                (flash, re).into_response()
            }
            EditResult::Error { id, msg, ui } => {
                let view: String = EditTemplate::new(
                    &[],
                    Some(msg),
                    Contact {
                        id: id as i64,
                        name: ui.name,
                        email: ui.email,
                    },
                )
                .render()
                .unwrap();
                Html::from(view).into_response()
            }
        }
    }
}

async fn delete_contact(State(state): State<AppState>, Path(id): Path<u32>) -> Redirect {
    let res = state.db.remove_contact(id).await.unwrap();
    Redirect::to("/contacts")
}

// #[serde_as]
#[derive(Debug, Deserialize)]
struct ContactSearch {
    // #[serde_as(as = "NoneAsEmptyString")]
    name: String,
}

async fn home(
    State(state): State<AppState>,
    flashes: IncomingFlashes,
    q: Option<Query<ContactSearch>>,
) -> (IncomingFlashes, Html<String>) {
    println!("{:?}", q);
    let contacts = if let Some(q) = q {
        state.db.search_by_name(&q.name).await.unwrap()
    } else {
        println!("serving all contacts");
        state.db.get_all_contacts().await.unwrap()
    };
    let messages: Box<_> = flashes.iter().map(|(_, text)| text).collect();
    let view = ContactsTemplate {
        messages: &messages,
        contacts,
    };
    let body = match view.render() {
        Ok(html) => html.into(),
        Err(e) => format!("failed to render ViewTemplate\n{:?}", e).into(),
    };
    (flashes, body)
}

async fn index() -> Redirect {
    Redirect::permanent("/contacts")
}

use futures_util::stream;
use std::{io, str::FromStr};
async fn download_archive(State(state): State<AppState>) -> impl IntoResponse {
    let chunks = state
        .db
        .get_all_contacts()
        .await
        .unwrap()
        .into_iter()
        .map(|c| io::Result::Ok(format!("name: '{}'\temail: '{}'\n", c.name, c.email)));
    let stream = stream::iter(chunks);

    let headers = [
        (header::CONTENT_TYPE, "text/toml; charset=utf-8"),
        (
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"contacts.txt\"",
        ),
    ];
    (headers, StreamBody::new(stream))
}

async fn handler_404() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "nothing to see here")
}

#[derive(Clone)]
struct AppState {
    db: DB,
    flash_config: axum_flash::Config,
}
impl FromRef<AppState> for axum_flash::Config {
    fn from_ref(state: &AppState) -> Self {
        state.flash_config.clone()
    }
}

async fn set_flash(flash: Flash) -> (Flash, Redirect) {
    (
        // The flash must be returned so the cookie is set
        flash.debug("Hi from flash!"),
        Redirect::to("/"),
    )
}

#[tokio::main]
async fn main() {
    let db = DB::new(5).await;
    let app_state = AppState {
        db,
        // The key should probably come from configuration
        flash_config: axum_flash::Config::new(Key::generate()),
    };

    // inject db connection into our routes
    // let home = {
    //     let db = db.clone();
    //     async move || home(db).await
    // };

    let app = Router::new()
        .route("/", get(index))
        .route("/contacts", get(home))
        .route("/contacts/download", get(download_archive))
        .route("/contacts/new", get(get_new))
        .route("/contacts/new", post(post_new))
        .route("/contacts/:id/edit", get(get_edit))
        .route("/contacts/:id/edit", post(post_edit))
        .route("/contacts/:id", delete(delete_contact))
        .route("/contacts/:id", get(view))
        .route("/set_flash", get(set_flash))
        .fallback(handler_404)
        .with_state(app_state);

    // build our application
    // run it with hyper on localhost:3000
    let adress = "0.0.0.0:3000";
    println!("starting server");
    axum::Server::bind(&adress.parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

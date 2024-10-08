mod actuators;
mod database;
mod sensors;

#[cfg(feature = "real-sensors")]
mod i2c;

#[cfg(feature = "real-sensors")]
use rppal::i2c::I2c;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use database::Database;
use futures::StreamExt;
use nmea_parser::ParsedMessage;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use zbus::{
    fdo,
    names::InterfaceName,
    zvariant,
    Connection,
};
use zvariant::OwnedValue;

#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::signal::{self};

const DEAD_TIMEOUT: u64 = 500;

#[cfg(feature = "real-sensors")]
fn init_i2c() -> anyhow::Result<Arc<Mutex<I2c>>> {
    // Préparation du BUS I2C
    println!("[I2C] Préparation du bus ...");
    let i2c_bus = I2c::new();
    if let Err(e) = i2c_bus {
        panic!("[I2C] Erreur de bus : {}", e);
    }
    let i2c_bus = Arc::new(Mutex::new(i2c_bus.unwrap()));
    println!("[I2C] Bus initialisé.");
    Ok(i2c_bus)
}

#[cfg(feature = "fake-sensors")]
fn init_i2c() -> anyhow::Result<bool> {
    Ok(true)
}

#[tokio::main]
async fn main() {
    let token = CancellationToken::new();
    let i2c_bus = init_i2c().unwrap();

    // Préparation de la base de donnée
    println!("[DB] Connexion à la base de donnée ...");
    let db = match Database::new().await {
        Ok(db) => {
            println!("[DB] Connexion établie.");
            Arc::new(db)
        }
        Err(e) => {
            panic!("[DB] Erreur de connexion: {}", e);
        }
    };

    // Capteur: GPS
    {
        let token = token.child_token();
        let mut reader = sensors::gps::Reader::new(token.clone()).unwrap();
        let db: Arc<Database> = db.clone();
        tokio::spawn(async move {
            while !token.is_cancelled() {
                if let Some(nmea) = reader.next().await {
                    match nmea {
                        ParsedMessage::Gga(gga) => {
                            if let Err(e) = db.send_gps_gga(gga).await {
                                println!("Erreur lors de la requête : {}", e);
                            }

                            // println!("Source:    {}",     gga.source);
                            // println!("Latitude:  {:.3}°", gga.latitude.unwrap_or(0.0));
                            // println!("Longitude: {:.3}°", gga.longitude.unwrap_or(0.0));
                            // println!("Satelites: {}", gga.satellite_count.unwrap_or(0));
                            // println!("Fix?: {}",  gga.quality == GgaQualityIndicator::GpsFix);
                            // println!("");
                        }
                        ParsedMessage::Vtg(vtg) => {
                            if let Err(e) = db.send_gps_vtg(vtg).await {
                                println!("Erreur lors de la requête : {}", e);
                            }
                        }
                        _ => {
                            // println!("Trame NMEA Inconnue.");
                        }
                    }
                }
            }
        });
    }

    // Capteur: IMU
    {
        let token = token.child_token();

        #[cfg(feature = "real-sensors")]
        let mut reader = sensors::imu::reader::Reader::new(i2c_bus.clone(), token.clone()).unwrap();

        #[cfg(feature = "fake-sensors")]
        let mut reader = sensors::imu::reader::Reader::new(token.clone()).unwrap();

        let db = db.clone();
        tokio::spawn(async move {
            while !token.is_cancelled() {
                if let Some(data) = reader.next().await {
                    //println!("Angles: {:?} T: {}°C", data.angles, data.temp);
                    if let Err(e) = db.send_imu(data).await {
                        println!("Erreur lors de la requête : {}", e);
                    }
                }

                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
    }

    // Capteur: Analog
    {
        let token = token.child_token();

        #[cfg(feature = "real-sensors")]
        let mut reader =
            sensors::analog::reader::Reader::new(i2c_bus.clone(), token.clone()).unwrap();

        #[cfg(feature = "fake-sensors")]
        let mut reader = sensors::analog::reader::Reader::new(token.clone()).unwrap();

        let db = db.clone();
        tokio::spawn(async move {
            while !token.is_cancelled() {
                if let Some(data) = reader.next().await {
                    if let Ok(data) = data {
                        //println!("BATT: {} V", data.battery);
                        if let Err(e) = db.send_analog(data).await {
                            println!("Erreur lors de la requête : {}", e);
                        }
                    }

                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        });
    }

    // Capteur: MAG
    {
        let token = token.child_token();

        #[cfg(feature = "real-sensors")]
        let mut reader = sensors::mag::reader::Reader::new(i2c_bus.clone(), token.clone()).unwrap();

        #[cfg(feature = "fake-sensors")]
        let mut reader = sensors::mag::reader::Reader::new(token.clone()).unwrap();

        let db = db.clone();
        tokio::spawn(async move {
            while !token.is_cancelled() {
                if let Some(data) = reader.next().await {
                    if let Ok(data) = data {
                        // println!(
                        //     "MAG: {} => ({},{},{})",
                        //     data.heading, data.raw.0, data.raw.1, data.raw.2
                        // );
                        if let Err(e) = db.send_mag(data).await {
                            println!("Erreur lors de la requête : {}", e);
                        }
                    }
                }

                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        });
    }

    // Modem 4G
    {
        let token = token.child_token();
        let db = db.clone();

        #[cfg(feature = "real-sensors")]
        {
            let connection = Connection::system()
                .await
                .expect("Impossible de gérer le D-BUS");

            tokio::spawn(async move {
                let proxy = fdo::PropertiesProxy::builder(&connection)
                    .destination("org.freedesktop.ModemManager1")
                    .expect("Destination invalide")
                    .path("/org/freedesktop/ModemManager1/Modem/0")
                    .expect("Path invalide")
                    .build()
                    .await
                    .expect("Impossible de créer le proxy pour la propriété");

                while !token.is_cancelled() {
                    let signal_quality: OwnedValue = proxy
                        .get(
                            InterfaceName::try_from("org.freedesktop.ModemManager1.Modem")
                                .expect("Type invalide"),
                            "SignalQuality",
                        )
                        .await
                        .expect("Impossible de récupérer la valeur SignalQuality.");

                    let signal = <(u32, bool)>::try_from(signal_quality).unwrap_or((0, false));

                    let _ = db.send_modem(signal.0).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            });
        }

        #[cfg(feature = "fake-sensors")]
        {
            tokio::spawn(async move {
                let mut rng = rand::thread_rng();

                while !token.is_cancelled() {
                    let signal: u32 = rng.gen();
                    let _ = db.send_modem(signal).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            });
        }
    }

    // Control
    {
        let token = token.child_token();
        let db = db.clone();
        tokio::spawn(async move {
            #[cfg(feature = "real-actuators")]
            {
                let motor = crate::actuators::motor::Motor::new();
                if let Err(e) = motor {
                    println!("[CONTROL] Erreur lors de l'init moteur: {}", e);
                    return;
                }

                let mut motor = motor.unwrap();

                let steer = crate::actuators::steering::Steering::new();
                if let Err(e) = steer {
                    println!("[CONTROL] Erreur lors de l'init steering: {}", e);
                    return;
                }
                let mut steer = steer.unwrap();

                while !token.is_cancelled() {
                    let stream = db.live_control().await;

                    match stream {
                        Ok(mut s) => {
                            while !token.is_cancelled() {
                                let control =
                                    timeout(Duration::from_millis(DEAD_TIMEOUT), s.next()).await;
                                match control {
                                    Ok(data) => {
                                        if data.is_none() {
                                            continue;
                                        }

                                        let data = data.unwrap();
                                        match data {
                                            Ok(data) => {
                                                if data.action != surrealdb::Action::Update {
                                                    continue;
                                                }

                                                if let Err(e) = steer.set_steer(data.data.steer) {
                                                    eprintln!("[CONTROL] Erreur lors du contrôle de la direction: {}", e)
                                                }

                                                if let Err(e) = motor.set_speed(data.data.speed) {
                                                    eprintln!("[CONTROL] Erreur lors du contrôle moteur: {}", e)
                                                }
                                            }

                                            Err(e) => {
                                                eprintln!(
                                                    "[CONTROL] Erreur lors de l'update: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        eprintln!("[CONTROL] Update tardif des données...");
                                        let _ = motor.set_speed(0.0);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[CONTROL] Erreur lors de la création du live: {}", e);
                        }
                    }
                }

                motor.safe_stop();
                steer.safe_stop();
            }

            #[cfg(feature = "fake-actuators")]
            {
                while !token.is_cancelled() {
                    let stream = db.live_control().await;

                    match stream {
                        Ok(mut s) => {
                            while !token.is_cancelled() {
                                let control =
                                    timeout(Duration::from_millis(DEAD_TIMEOUT), s.next()).await;
                                match control {
                                    Ok(data) => {
                                        if data.is_none() {
                                            continue;
                                        }

                                        let data = data.unwrap();
                                        match data {
                                            Ok(data) => {
                                                if data.action != surrealdb::Action::Update {
                                                    continue;
                                                }

                                                println!(
                                                    "[CONTROL] Steer: {} Speed: {}",
                                                    data.data.steer, data.data.speed
                                                );
                                            }

                                            Err(e) => {
                                                eprintln!(
                                                    "[CONTROL] Erreur lors de l'update: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        eprintln!("[CONTROL] Update tardif des données...");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[CONTROL] Erreur lors de la création du live: {}", e);
                        }
                    }
                }
            }
        });
    }

    #[cfg(unix)]
    {
        let mut test = tokio::signal::unix::signal(SignalKind::interrupt()).unwrap();
        tokio::select! {
            _ = test.recv() => {
                println!("Signal d'interruption reçu");
                token.cancel();
            },
            _ = signal::ctrl_c() => {
                println!("Signal de contrôle C reçu");
                token.cancel();
            },
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = signal::ctrl_c() => {
                println!("Signal de contrôle C reçu");
                token.cancel();
            },
        }
    }
}

- update logger (log.info, log.error, log.ciritalError)
- custom download dir
- add ablity to download favs

every 5 min chekc fav 
if we see all new videos at the top then scroll until we dont or reach the end of the page 
once we have all the new fav videos add to seen videos.json with fav flag,
it might already be there so if its already there then just change the flag, and make the hardlink 
if its not there then just add it 
when we donwlaod stuff we will update the downlaod status and then make the hardlink
